use futures_util::{stream, StreamExt, TryStreamExt};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    fs,
    io::{self, Cursor},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use zip::ZipArchive;

use crate::{config, settings};

#[derive(Debug, Deserialize)]
pub struct VersionManifest {
    pub versions: Vec<ManifestVersion>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestVersion {
    pub id: String,
    pub url: String,
    pub sha1: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMetadata {
    pub id: String,
    pub main_class: String,
    pub inherits_from: Option<String>,
    pub arguments: Option<LaunchArguments>,
    pub minecraft_arguments: Option<String>,
    pub libraries: Vec<Library>,
    pub downloads: Option<VersionDownloads>,
    pub asset_index: Option<AssetIndex>,
    pub assets: Option<String>,
    pub java_version: Option<JavaVersion>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LaunchArguments {
    pub game: Vec<Argument>,
    pub jvm: Vec<Argument>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Value(String),
    Values(Vec<String>),
    Conditional(ConditionalArgument),
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConditionalArgument {
    pub rules: Vec<Rule>,
    pub value: ArgumentValue,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    Value(String),
    Values(Vec<String>),
}

#[derive(Clone, Debug, Deserialize)]
pub struct Rule {
    pub action: String,
    pub os: Option<OsRule>,
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibraryDownloads>,
    pub rules: Option<Vec<Rule>>,
    pub natives: Option<HashMap<String, String>>,
    pub extract: Option<ExtractRules>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LibraryDownloads {
    pub artifact: Option<Artifact>,
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub path: Option<String>,
    pub url: String,
    pub size: Option<u64>,
    pub sha1: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExtractRules {
    pub exclude: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VersionDownloads {
    pub client: Artifact,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AssetIndex {
    pub id: String,
    pub url: String,
    pub size: u64,
    pub sha1: String,
}

#[derive(Debug, Deserialize)]
pub struct AssetIndexContents {
    pub objects: HashMap<String, AssetObject>,
}

#[derive(Debug, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
    pub major_version: u32,
}

pub async fn install_vanilla(app: &AppHandle, game_dir: &Path) -> Result<VersionMetadata, String> {
    emit_progress(app, "Получение официальных метаданных Minecraft…")?;
    let client = http_client()?;
    let manifest: VersionManifest = client
        .get(config::MOJANG_VERSION_MANIFEST_URL)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Не удалось получить манифест Mojang: {error}"))?
        .json()
        .await
        .map_err(|error| format!("Манифест Mojang имеет неверный формат: {error}"))?;
    let entry = manifest
        .versions
        .iter()
        .find(|entry| entry.id == config::MINECRAFT_VERSION)
        .ok_or_else(|| {
            format!(
                "Версия Minecraft {} отсутствует в манифесте Mojang",
                config::MINECRAFT_VERSION
            )
        })?;
    let metadata_bytes = fetch_bytes(&client, &entry.url).await?;
    verify_sha1(&metadata_bytes, &entry.sha1, "метаданных Minecraft")?;
    let metadata: VersionMetadata = serde_json::from_slice(&metadata_bytes)
        .map_err(|error| format!("Метаданные Minecraft повреждены: {error}"))?;
    if metadata.id != config::MINECRAFT_VERSION {
        return Err(format!(
            "Ожидалась версия Minecraft {}, получена {}",
            config::MINECRAFT_VERSION,
            metadata.id
        ));
    }
    let java_version = metadata
        .java_version
        .as_ref()
        .ok_or_else(|| "В метаданных Minecraft отсутствует требование к Java".to_string())?;
    if java_version.major_version != config::JAVA_MAJOR_VERSION {
        return Err(format!(
            "Для Minecraft {} требуется Java {} ({}), конфигурация лаунчера рассчитана на Java {}",
            metadata.id,
            java_version.major_version,
            java_version.component,
            config::JAVA_MAJOR_VERSION
        ));
    }
    if metadata.arguments.is_none() || metadata.minecraft_arguments.is_some() {
        return Err(format!(
            "Формат аргументов Minecraft {} не поддерживается этим минимальным лаунчером",
            metadata.id
        ));
    }

    let version_dir = game_dir.join("versions").join(&metadata.id);
    fs::create_dir_all(&version_dir)
        .map_err(|error| format!("Не удалось создать каталог Minecraft: {error}"))?;
    let metadata_path = version_dir.join(format!("{}.json", metadata.id));
    write_atomic(&metadata_path, &metadata_bytes)
        .map_err(|error| format!("Не удалось сохранить метаданные Minecraft: {error}"))?;

    let client_download = metadata
        .downloads
        .as_ref()
        .ok_or_else(|| "В метаданных Minecraft отсутствует клиентский JAR".to_string())?
        .client
        .clone();
    let client_path = version_dir.join(format!("{}.jar", metadata.id));
    ensure_artifact(&client, &client_download, &client_path, "клиент Minecraft").await?;

    emit_progress(app, "Подготовка библиотек Minecraft…")?;
    let libraries_dir = game_dir.join("libraries");
    let natives_dir = settings::settings_directory(app)?
        .join("cache")
        .join(format!("natives-{}", config::NEOFORGE_PROFILE_ID));
    fs::create_dir_all(&natives_dir)
        .map_err(|error| format!("Не удалось подготовить папку нативных библиотек: {error}"))?;

    let mut downloads = Vec::<(Artifact, PathBuf, String, bool, Vec<String>)>::new();
    for library in &metadata.libraries {
        if !supported_library(library) {
            continue;
        }
        let exclusions = library
            .extract
            .as_ref()
            .and_then(|rules| rules.exclude.clone())
            .unwrap_or_default();
        let separate_native = library
            .name
            .split(':')
            .nth(3)
            .is_some_and(|classifier| classifier == "natives-windows");
        if let Some(downloads_metadata) = &library.downloads {
            if let Some(artifact) = &downloads_metadata.artifact {
                let path = artifact
                    .path
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or(coordinate_path(&library.name)?);
                downloads.push((
                    artifact.clone(),
                    libraries_dir.join(path),
                    library.name.clone(),
                    separate_native,
                    exclusions.clone(),
                ));
            }
            if let Some(classifier) = library
                .natives
                .as_ref()
                .and_then(|natives| natives.get("windows"))
                .map(|value| value.replace("${arch}", "64"))
            {
                let artifact = downloads_metadata
                    .classifiers
                    .as_ref()
                    .and_then(|classifiers| classifiers.get(&classifier))
                    .ok_or_else(|| {
                        format!(
                            "В метаданных отсутствует Windows native-библиотека {classifier} ({})",
                            library.name
                        )
                    })?;
                let path = artifact
                    .path
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or(coordinate_path(&library.name)?);
                downloads.push((
                    artifact.clone(),
                    libraries_dir.join(path),
                    library.name.clone(),
                    true,
                    exclusions,
                ));
            }
        } else if library
            .natives
            .as_ref()
            .is_some_and(|natives| natives.contains_key("windows"))
        {
            return Err(format!(
                "Для библиотеки {} отсутствуют данные загрузки native-файла",
                library.name
            ));
        }
    }
    let work = downloads
        .into_iter()
        .map(|(artifact, path, label, is_native, exclusions)| {
            let client = client.clone();
            let natives_dir = natives_dir.clone();
            async move {
                ensure_artifact(&client, &artifact, &path, &label).await?;
                if is_native {
                    extract_natives(&path, &natives_dir, &exclusions)?;
                }
                Ok::<(), String>(())
            }
        });
    stream::iter(work)
        .buffer_unordered(12)
        .try_collect::<Vec<_>>()
        .await?;

    let asset_index = metadata
        .asset_index
        .as_ref()
        .ok_or_else(|| "В метаданных Minecraft отсутствует индекс ресурсов".to_string())?;
    if metadata
        .assets
        .as_deref()
        .is_some_and(|assets| assets != asset_index.id)
    {
        return Err("Идентификатор assets не совпадает с индексом ресурсов Minecraft".into());
    }
    let assets_dir = game_dir.join("assets");
    let index_path = assets_dir
        .join("indexes")
        .join(format!("{}.json", asset_index.id));
    ensure_artifact(
        &client,
        &Artifact {
            path: None,
            url: asset_index.url.clone(),
            size: Some(asset_index.size),
            sha1: Some(asset_index.sha1.clone()),
        },
        &index_path,
        "индекса ресурсов Minecraft",
    )
    .await?;
    let index_bytes = fs::read(&index_path)
        .map_err(|error| format!("Не удалось прочитать индекс ресурсов: {error}"))?;
    let index: AssetIndexContents = serde_json::from_slice(&index_bytes)
        .map_err(|error| format!("Индекс ресурсов Minecraft повреждён: {error}"))?;
    emit_progress(
        app,
        &format!(
            "Загрузка ресурсов Minecraft ({} файлов)…",
            index.objects.len()
        ),
    )?;

    let asset_tasks = index.objects.into_values().map(|object| {
        let client = client.clone();
        let path_prefix = object.hash.get(..2).unwrap_or_default().to_string();
        let hash = object.hash.clone();
        let destination = assets_dir.join("objects").join(&path_prefix).join(&hash);
        let url = format!("https://resources.download.minecraft.net/{path_prefix}/{hash}");
        async move {
            ensure_artifact(
                &client,
                &Artifact {
                    path: None,
                    url,
                    size: Some(object.size),
                    sha1: Some(object.hash),
                },
                &destination,
                "ресурса Minecraft",
            )
            .await
        }
    });
    stream::iter(asset_tasks)
        .buffer_unordered(16)
        .try_collect::<Vec<_>>()
        .await?;

    Ok(metadata)
}

pub fn library_allowed(rules: Option<&[Rule]>) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule) {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

pub fn supported_library(library: &Library) -> bool {
    if !library_allowed(library.rules.as_deref()) {
        return false;
    }
    let Some(classifier) = library.name.split(':').nth(3) else {
        return true;
    };
    !classifier.starts_with("natives-windows") || classifier == "natives-windows"
}

fn rule_matches(rule: &Rule) -> bool {
    if let Some(os) = &rule.os {
        if os.name.as_deref().is_some_and(|name| name != "windows")
            || os
                .arch
                .as_deref()
                .is_some_and(|arch| arch != "x86_64" && arch != "amd64")
        {
            return false;
        }
        if os
            .version
            .as_deref()
            .is_some_and(|pattern| !windows_version_matches(pattern))
        {
            return false;
        }
    }
    rule.features
        .as_ref()
        .is_none_or(|features| features.values().all(|expected| !expected))
}

fn windows_version_matches(pattern: &str) -> bool {
    pattern == ".*"
}

pub fn coordinate_path(coordinate: &str) -> Result<PathBuf, String> {
    let mut parts = coordinate.split('@');
    let coordinate = parts.next().unwrap_or_default();
    let extension = parts.next().unwrap_or("jar");
    if parts.next().is_some() {
        return Err(format!("Некорректная координата библиотеки {coordinate}"));
    }
    let mut components = coordinate.split(':');
    let group = components.next().unwrap_or_default();
    let artifact = components.next().unwrap_or_default();
    let version = components.next().unwrap_or_default();
    let classifier = components.next();
    if group.is_empty() || artifact.is_empty() || version.is_empty() || components.next().is_some()
    {
        return Err(format!("Некорректная координата библиотеки {coordinate}"));
    }
    let group_path = group.replace('.', "/");
    let filename = match classifier {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.{extension}"),
        None => format!("{artifact}-{version}.{extension}"),
    };
    Ok(PathBuf::from(group_path)
        .join(artifact)
        .join(version)
        .join(filename))
}

pub fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|error| format!("Не удалось создать HTTP-клиент: {error}"))
}

pub async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err(format!("Отклонён небезопасный URL метаданных: {url}"));
    }
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Не удалось скачать {url}: {error}"))?;
    if response.url().scheme() != "https" {
        return Err(format!(
            "Загрузка перенаправлена на незащищённый URL: {url}"
        ));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| format!("Загрузка {url} прервалась: {error}"))
}

async fn ensure_artifact(
    client: &reqwest::Client,
    artifact: &Artifact,
    destination: &Path,
    label: &str,
) -> Result<(), String> {
    if destination.is_file() && verify_file(destination, artifact).is_ok() {
        return Ok(());
    }
    let bytes = fetch_bytes(client, &artifact.url).await?;
    verify_bytes(&bytes, artifact, label)?;
    write_atomic(destination, &bytes).map_err(|error| {
        format!(
            "Не удалось сохранить {label} {}: {error}",
            destination.display()
        )
    })
}

fn verify_file(path: &Path, artifact: &Artifact) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    verify_bytes(&bytes, artifact, "локального файла")
}

pub fn artifact_is_valid(path: &Path, artifact: &Artifact) -> bool {
    verify_file(path, artifact).is_ok()
}

fn verify_bytes(bytes: &[u8], artifact: &Artifact, label: &str) -> Result<(), String> {
    if let Some(size) = artifact.size {
        if bytes.len() as u64 != size {
            return Err(format!(
                "Неверный размер {label}: ожидалось {size}, получено {}",
                bytes.len()
            ));
        }
    }
    if let Some(expected) = &artifact.sha1 {
        verify_sha1(bytes, expected, label)?;
    }
    if artifact.size.is_none() && artifact.sha1.is_none() {
        return Err(format!("Для {label} нет ни контрольной суммы, ни размера"));
    }
    Ok(())
}

fn verify_sha1(bytes: &[u8], expected: &str, label: &str) -> Result<(), String> {
    let actual = hex::encode(Sha1::digest(bytes));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "SHA-1 {label} не совпал: ожидалось {expected}, получено {actual}"
        ))
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "путь файла не имеет родительской папки",
        )
    })?;
    fs::create_dir_all(parent)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{stamp}", std::process::id()));
    fs::write(&temporary, bytes)?;
    replace_file(&temporary, path)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn extract_natives(path: &Path, destination: &Path, exclusions: &[String]) -> Result<(), String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("Не удалось прочитать native-библиотеку: {error}"))?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("Native-библиотека повреждена: {error}"))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Не удалось прочитать native-библиотеку: {error}"))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err("Native-библиотека содержит небезопасный путь".into());
        };
        let name = relative.to_string_lossy().replace('\\', "/");
        if name.starts_with("META-INF/")
            || exclusions.iter().any(|excluded| name.starts_with(excluded))
            || entry.is_dir()
        {
            continue;
        }
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Не удалось создать папку native-библиотеки: {error}"))?;
        }
        let mut output = fs::File::create(&target)
            .map_err(|error| format!("Не удалось создать native-файл: {error}"))?;
        io::copy(&mut entry, &mut output)
            .map_err(|error| format!("Не удалось распаковать native-файл: {error}"))?;
    }
    Ok(())
}

fn emit_progress(app: &AppHandle, message: &str) -> Result<(), String> {
    app.emit(
        "launcher-progress",
        Progress {
            message: message.to_string(),
        },
    )
    .map_err(|error| format!("Не удалось обновить статус лаунчера: {error}"))
}

#[derive(serde::Serialize, Clone)]
struct Progress {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::{coordinate_path, library_allowed, supported_library, Library, Rule};
    use std::collections::HashMap;

    #[test]
    fn maven_coordinates_map_to_repository_relative_paths() {
        assert_eq!(
            coordinate_path("org.lwjgl:lwjgl:3.3.3:natives-windows@zip").unwrap(),
            std::path::PathBuf::from("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.zip")
        );
    }

    #[test]
    fn library_rules_select_only_matching_windows_x64_entries() {
        let allow = Rule {
            action: "allow".into(),
            os: Some(super::OsRule {
                name: Some("windows".into()),
                arch: Some("x86_64".into()),
                version: None,
            }),
            features: None,
        };
        assert!(library_allowed(Some(&[allow])));
        let feature_gated = Rule {
            action: "allow".into(),
            os: None,
            features: Some(HashMap::from([("is_demo_user".into(), true)])),
        };
        assert!(!library_allowed(Some(&[feature_gated])));
    }

    #[test]
    fn rejects_non_x64_windows_native_classifiers() {
        let arm_native = Library {
            name: "org.lwjgl:lwjgl:3.3.3:natives-windows-arm64".into(),
            downloads: None,
            rules: None,
            natives: None,
            extract: None,
        };
        let x64_native = Library {
            name: "org.lwjgl:lwjgl:3.3.3:natives-windows".into(),
            ..arm_native.clone()
        };
        assert!(!supported_library(&arm_native));
        assert!(supported_library(&x64_native));
    }
}
