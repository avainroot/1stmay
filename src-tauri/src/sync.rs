use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::{config, settings};

const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_MOD_FILE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct RemoteManifest {
    mods: Vec<RemoteMod>,
}

#[derive(Clone, Debug, Deserialize)]
struct RemoteMod {
    name: String,
    size: u64,
    sha256: String,
    path: String,
}

#[derive(Clone, Debug)]
struct ManifestFile {
    path: String,
    size: u64,
    sha256: String,
    remote_path: String,
}

pub async fn synchronize_mods(app: &AppHandle, game_dir: &Path) -> Result<(), String> {
    let manifest_url = Url::parse(config::MOD_MANIFEST_URL)
        .map_err(|error| format!("URL манифеста настроен неверно: {error}"))?;
    if !matches!(manifest_url.scheme(), "https" | "http") {
        return Err("URL манифеста должен использовать HTTP или HTTPS".into());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|error| format!("Не удалось подготовить HTTP-клиент: {error}"))?;
    let app_dir = settings::settings_directory(app)?;
    fs::create_dir_all(&app_dir)
        .map_err(|error| format!("Не удалось создать папку данных лаунчера: {error}"))?;
    let managed_file = app_dir.join("managed-mods.json");
    synchronize_from_url(&client, &manifest_url, game_dir, &managed_file, |message| {
        emit_progress(app, message)
    })
    .await
}

async fn synchronize_from_url<F>(
    client: &reqwest::Client,
    manifest_url: &Url,
    game_dir: &Path,
    managed_file: &Path,
    mut progress: F,
) -> Result<(), String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    progress("Сверка списка модов с сервером…")?;
    let response = client
        .get(manifest_url.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Не удалось получить манифест модов: {error}"))?;
    if response.url().origin() != manifest_url.origin()
        || response.url().scheme() != manifest_url.scheme()
    {
        return Err("Загрузка манифеста была перенаправлена на другой сервер".into());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return Err("Размер манифеста модов превышает допустимые 4 МиБ".into());
    }
    let manifest_bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Загрузка манифеста модов прервалась: {error}"))?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err("Размер манифеста модов превышает допустимые 4 МиБ".into());
    }
    let manifest = parse_manifest(&manifest_bytes)?;
    validate_manifest(&manifest)?;

    let game_root = fs::canonicalize(game_dir)
        .map_err(|error| format!("Не удалось определить папку игры: {error}"))?;
    let mods_dir = game_root.join("mods");
    fs::create_dir_all(&mods_dir)
        .map_err(|error| format!("Не удалось создать папку mods: {error}"))?;
    let canonical_mods = fs::canonicalize(&mods_dir)
        .map_err(|error| format!("Не удалось проверить папку mods: {error}"))?;
    if !canonical_mods.starts_with(&game_root) {
        return Err("Папка mods ведёт за пределы каталога игры".into());
    }

    let previous = load_managed_paths(managed_file)?;
    let mut current = HashSet::new();
    for entry in &manifest {
        let path = safe_target(&canonical_mods, &entry.path)?;
        if file_matches(&path, entry)? {
            current.insert(entry.path.to_lowercase());
            continue;
        }
        let url = mod_url(manifest_url, &entry.remote_path)?;
        progress(&format!("Загрузка мода {}…", entry.path))?;
        download_mod(client, &url, &path, entry).await?;
        current.insert(entry.path.to_lowercase());
    }

    for stale in previous
        .iter()
        .filter(|path| !current.contains(&path.to_lowercase()))
    {
        let path = safe_target(&canonical_mods, stale)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "Устаревший мод является ссылкой, удаление запрещено: {stale}"
                ));
            }
            Ok(metadata) if metadata.is_file() => fs::remove_file(&path)
                .map_err(|error| format!("Не удалось удалить устаревший мод {stale}: {error}"))?,
            Ok(_) => return Err(format!("Устаревший путь мода не является файлом: {stale}")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Не удалось проверить устаревший мод {stale}: {error}"
                ))
            }
        }
    }

    let paths = manifest
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    write_atomic(
        managed_file,
        &serde_json::to_vec_pretty(&paths)
            .map_err(|error| format!("Не удалось сохранить список управляемых модов: {error}"))?,
    )
    .map_err(|error| format!("Не удалось сохранить состояние синхронизации модов: {error}"))?;
    Ok(())
}

fn parse_manifest(bytes: &[u8]) -> Result<Vec<ManifestFile>, String> {
    let remote: RemoteManifest = serde_json::from_slice(bytes)
        .map_err(|error| format!("Манифест модов повреждён или имеет неверный формат: {error}"))?;
    remote
        .mods
        .into_iter()
        .map(|entry| {
            validate_relative_path(&entry.name)?;
            if entry.name.contains('/') {
                return Err(format!(
                    "Имя мода должно быть именем файла без каталогов: {}",
                    entry.name
                ));
            }
            let decoded = percent_decode_path(&entry.path)?;
            validate_relative_path(&decoded)?;
            let filename = decoded
                .rsplit('/')
                .next()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("В пути мода нет имени файла: {}", entry.path))?;
            if filename != entry.name {
                return Err(format!(
                    "Имя мода {} не совпадает с путём {}",
                    entry.name, entry.path
                ));
            }
            Ok(ManifestFile {
                path: entry.name,
                size: entry.size,
                sha256: entry.sha256,
                remote_path: entry.path,
            })
        })
        .collect()
}

fn percent_decode_path(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(format!("Повреждён percent-encoding в пути мода: {value}"));
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| format!("Повреждён percent-encoding в пути мода: {value}"))?;
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| format!("Повреждён percent-encoding в пути мода: {value}"))?;
            decoded.push(byte);
            index += 3;
        } else if bytes[index] == b'+' {
            decoded.push(b'+');
            index += 1;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| format!("Путь мода содержит недопустимую кодировку: {value}"))
}

fn validate_manifest(manifest: &[ManifestFile]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for entry in manifest {
        validate_relative_path(&entry.path)?;
        let normalized = entry.path.to_lowercase();
        if !seen.insert(normalized) {
            return Err(format!(
                "Манифест содержит повторяющийся путь: {}",
                entry.path
            ));
        }
        if entry.sha256.len() != 64 || !entry.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!(
                "Для файла {} указан недопустимый SHA-256",
                entry.path
            ));
        }
        if entry.size > MAX_MOD_FILE_BYTES {
            return Err(format!("Размер мода {} превышает лимит 1 ГиБ", entry.path));
        }
    }
    Ok(())
}

pub fn validate_relative_path(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains(':')
        || value.contains('\0')
    {
        return Err(format!("Недопустимый относительный путь мода: {value:?}"));
    }
    let components = Path::new(value).components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("Недопустимый путь мода: {value:?}"));
    }
    for segment in value.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.ends_with('.')
            || segment.ends_with(' ')
            || is_windows_reserved_name(segment)
        {
            return Err(format!("Недопустимый компонент пути мода: {segment:?}"));
        }
    }
    Ok(())
}

fn is_windows_reserved_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
        })
}

fn safe_target(root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_relative_path(relative)?;
    let mut target = root.to_path_buf();
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        target.push(part);
        if parts.peek().is_some() && target.exists() {
            let metadata = fs::symlink_metadata(&target)
                .map_err(|error| format!("Не удалось проверить путь мода {relative}: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "Путь мода пересекает недопустимый компонент: {relative}"
                ));
            }
            let canonical = fs::canonicalize(&target)
                .map_err(|error| format!("Не удалось проверить путь мода {relative}: {error}"))?;
            if !canonical.starts_with(root) {
                return Err(format!("Путь мода выходит за папку mods: {relative}"));
            }
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Путь мода является символической ссылкой: {relative}"
            ));
        }
        if !metadata.is_file() {
            return Err(format!("Путь мода не является файлом: {relative}"));
        }
    }
    Ok(target)
}

fn file_matches(path: &Path, expected: &ManifestFile) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "Не удалось проверить мод {}: {error}",
                expected.path
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Локальный путь мода {} является ссылкой или не является файлом",
            expected.path
        ));
    }
    if metadata.len() != expected.size {
        return Ok(false);
    }
    let mut file = fs::File::open(path)
        .map_err(|error| format!("Не удалось прочитать мод {}: {error}", expected.path))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Не удалось проверить хеш мода {}: {error}", expected.path))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()).eq_ignore_ascii_case(&expected.sha256))
}

fn mod_url(manifest_url: &Url, remote_path: &str) -> Result<Url, String> {
    if remote_path.is_empty()
        || remote_path.starts_with('/')
        || remote_path.starts_with("//")
        || remote_path.contains("://")
    {
        return Err(format!("Недопустимый URL-путь мода: {remote_path}"));
    }
    let host = manifest_url
        .host_str()
        .ok_or_else(|| "URL манифеста не содержит хост".to_string())?;
    let origin = match manifest_url.port() {
        Some(port) => format!("{}://{host}:{port}/", manifest_url.scheme()),
        None => format!("{}://{host}/", manifest_url.scheme()),
    };
    let base = Url::parse(&origin)
        .map_err(|error| format!("Не удалось определить адрес сервера модов: {error}"))?;
    let url = base
        .join(remote_path)
        .map_err(|error| format!("Не удалось построить URL загрузки мода: {error}"))?;
    if url.origin() != manifest_url.origin() || url.scheme() != manifest_url.scheme() {
        return Err("Путь мода указывает на другой сервер".into());
    }
    Ok(url)
}

async fn download_mod(
    client: &reqwest::Client,
    url: &Url,
    destination: &Path,
    expected: &ManifestFile,
) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Не удалось создать папку для {}: {error}", expected.path))?;
        let root = destination
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "mods"))
            .ok_or_else(|| "Не удалось определить управляемую папку mods".to_string())?;
        let canonical_parent = fs::canonicalize(parent).map_err(|error| {
            format!("Не удалось проверить папку для {}: {error}", expected.path)
        })?;
        if !canonical_parent.starts_with(root) {
            return Err(format!(
                "Путь мода выходит за папку mods: {}",
                expected.path
            ));
        }
    }

    let response = client
        .get(url.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Не удалось скачать мод {}: {error}", expected.path))?;
    if response.url().origin() != url.origin() || response.url().scheme() != url.scheme() {
        return Err(format!(
            "Загрузка мода {} была перенаправлена на другой сервер",
            expected.path
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size != expected.size)
    {
        return Err(format!(
            "Размер мода {} на сервере не совпадает с манифестом",
            expected.path
        ));
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("Системное время недоступно: {error}"))?
        .as_nanos();
    let temporary =
        destination.with_extension(format!("1stmay-{}-{stamp}.part", std::process::id()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|error| {
            format!(
                "Не удалось создать временный файл для {}: {error}",
                expected.path
            )
        })?;
    let result = write_and_verify_download(response, &mut file, expected).await;
    drop(file);
    if let Err(error) = result {
        return match tokio::fs::remove_file(&temporary).await {
            Ok(()) => Err(error),
            Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; не удалось удалить временный файл {}: {cleanup}",
                temporary.display()
            )),
        };
    }
    replace_file(&temporary, destination).map_err(|error| {
        format!(
            "Не удалось установить проверенный мод {}: {error}",
            expected.path
        )
    })
}

async fn write_and_verify_download(
    response: reqwest::Response,
    file: &mut tokio::fs::File,
    expected: &ManifestFile,
) -> Result<(), String> {
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| format!("Загрузка мода {} прервалась: {error}", expected.path))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > expected.size {
            return Err(format!(
                "Загруженный мод {} превышает размер из манифеста",
                expected.path
            ));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Не удалось записать мод {}: {error}", expected.path))?;
    }
    file.flush().await.map_err(|error| {
        format!(
            "Не удалось завершить запись мода {}: {error}",
            expected.path
        )
    })?;
    if downloaded != expected.size {
        return Err(format!(
            "Размер загруженного мода {} не совпадает: ожидалось {}, получено {downloaded}",
            expected.path, expected.size
        ));
    }
    let actual_hash = hex::encode(hasher.finalize());
    if !actual_hash.eq_ignore_ascii_case(&expected.sha256) {
        return Err(format!(
            "SHA-256 загруженного мода {} не совпал с манифестом",
            expected.path
        ));
    }
    Ok(())
}

fn load_managed_paths(path: &Path) -> Result<HashSet<String>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => {
            return Err(format!(
                "Не удалось прочитать список управляемых модов: {error}"
            ))
        }
    };
    let paths: Vec<String> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Список управляемых модов повреждён: {error}"))?;
    let mut validated = HashSet::new();
    for path in paths {
        validate_relative_path(&path)
            .map_err(|error| format!("Список управляемых модов содержит неверный путь: {error}"))?;
        validated.insert(path);
    }
    Ok(validated)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "у snapshot нет родительской папки",
        )
    })?;
    fs::create_dir_all(parent)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{stamp}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
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

fn emit_progress(app: &AppHandle, message: &str) -> Result<(), String> {
    app.emit(
        "launcher-progress",
        Progress {
            message: message.to_string(),
        },
    )
    .map_err(|error| format!("Не удалось обновить статус лаунчера: {error}"))
}

#[derive(Serialize, Clone)]
struct Progress {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::{
        file_matches, mod_url, parse_manifest, percent_decode_path, validate_manifest,
        validate_relative_path, ManifestFile,
    };
    use sha2::{Digest, Sha256};
    use std::{collections::HashMap, path::Path};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };
    use url::Url;

    #[test]
    fn accepts_safe_relative_paths_and_rejects_escape_paths() {
        assert!(validate_relative_path("library/mod-a.jar").is_ok());
        for path in [
            "",
            "../escape.jar",
            "mods/../../escape.jar",
            "/absolute.jar",
            "C:/drive.jar",
            "sub\\file.jar",
            "CON.jar",
            "folder/.",
            "folder/",
        ] {
            assert!(validate_relative_path(path).is_err(), "{path:?}");
        }
    }

    #[test]
    fn parses_live_mods_json_with_encoded_paths() {
        let hash = hex::encode(Sha256::digest(b"jar"));
        let parsed = parse_manifest(
            format!(
                r#"{{"mods":[{{"name":"[Neoforge]ctov.jar","size":3,"sha256":"{hash}","path":"minecraft/mods/%5BNeoforge%5Dctov.jar"}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(parsed[0].path, "[Neoforge]ctov.jar");
        assert_eq!(
            percent_decode_path(&parsed[0].remote_path).unwrap(),
            "minecraft/mods/[Neoforge]ctov.jar"
        );
        let url = mod_url(
            &Url::parse("https://remote.example/mods.json").unwrap(),
            &parsed[0].remote_path,
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://remote.example/minecraft/mods/%5BNeoforge%5Dctov.jar"
        );
    }

    #[test]
    fn validates_hashes_and_duplicate_paths_in_manifest() {
        let file = ManifestFile {
            path: "mod.jar".into(),
            size: 1,
            sha256: "a".repeat(64),
            remote_path: "minecraft/mods/mod.jar".into(),
        };
        assert!(validate_manifest(std::slice::from_ref(&file)).is_ok());
        assert!(validate_manifest(&[
            file.clone(),
            ManifestFile {
                path: "MOD.jar".into(),
                ..file
            }
        ])
        .is_err());
    }

    #[test]
    fn compares_local_file_size_and_sha256() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mod.jar");
        std::fs::write(&path, b"jar").unwrap();
        let file = ManifestFile {
            path: "mod.jar".into(),
            size: 3,
            sha256: hex::encode(Sha256::digest(b"jar")),
            remote_path: "minecraft/mods/mod.jar".into(),
        };
        assert!(file_matches(&path, &file).unwrap());
    }

    #[tokio::test]
    async fn synchronizes_added_changed_and_removed_mods_without_touching_unmanaged_files() {
        let temporary = tempfile::tempdir().unwrap();
        let game_dir = temporary.path().join("game");
        std::fs::create_dir_all(game_dir.join("mods")).unwrap();
        std::fs::write(game_dir.join("mods/user.jar"), b"user").unwrap();
        let managed_file = temporary.path().join("appdata/managed-mods.json");

        let (url, server) = serve(
            3,
            HashMap::from([
                (
                    "/mods.json".into(),
                    (manifest_bytes(&[("a.jar", b"one")]), None),
                ),
                ("/minecraft/mods/a.jar".into(), (b"one".to_vec(), Some(3))),
            ]),
        )
        .await;
        run_sync(&url, &game_dir, &managed_file).await.unwrap();
        server.abort();
        assert_eq!(std::fs::read(game_dir.join("mods/a.jar")).unwrap(), b"one");

        let (url, server) = serve(
            5,
            HashMap::from([
                (
                    "/mods.json".into(),
                    (
                        manifest_bytes(&[("a.jar", b"two"), ("b.jar", b"new")]),
                        None,
                    ),
                ),
                ("/minecraft/mods/a.jar".into(), (b"two".to_vec(), Some(3))),
                ("/minecraft/mods/b.jar".into(), (b"new".to_vec(), Some(3))),
            ]),
        )
        .await;
        run_sync(&url, &game_dir, &managed_file).await.unwrap();
        server.abort();
        assert_eq!(std::fs::read(game_dir.join("mods/a.jar")).unwrap(), b"two");
        assert_eq!(std::fs::read(game_dir.join("mods/b.jar")).unwrap(), b"new");

        let (url, server) = serve(
            1,
            HashMap::from([(
                "/mods.json".into(),
                (manifest_bytes(&[("b.jar", b"new")]), None),
            )]),
        )
        .await;
        run_sync(&url, &game_dir, &managed_file).await.unwrap();
        server.abort();
        assert!(!game_dir.join("mods/a.jar").exists());
        assert_eq!(
            std::fs::read(game_dir.join("mods/user.jar")).unwrap(),
            b"user"
        );
    }

    #[tokio::test]
    async fn corrupt_manifest_does_not_mutate_local_mods() {
        let temporary = tempfile::tempdir().unwrap();
        let game_dir = temporary.path().join("game");
        std::fs::create_dir_all(game_dir.join("mods")).unwrap();
        std::fs::write(game_dir.join("mods/old.jar"), b"old").unwrap();
        let managed_file = temporary.path().join("appdata/managed-mods.json");
        let (url, server) = serve(
            1,
            HashMap::from([("/mods.json".into(), (b"{not-json".to_vec(), None))]),
        )
        .await;
        assert!(run_sync(&url, &game_dir, &managed_file).await.is_err());
        server.abort();
        assert_eq!(
            std::fs::read(game_dir.join("mods/old.jar")).unwrap(),
            b"old"
        );
    }

    #[tokio::test]
    async fn interrupted_or_bad_hash_download_preserves_existing_file() {
        for (body, declared_size, expected) in [
            (b"bad".as_slice(), 8u64, b"download".as_slice()),
            (b"evil".as_slice(), 4u64, b"good".as_slice()),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let game_dir = temporary.path().join("game");
            std::fs::create_dir_all(game_dir.join("mods")).unwrap();
            std::fs::write(game_dir.join("mods/mod.jar"), b"old").unwrap();
            let managed_file = temporary.path().join("appdata/managed-mods.json");
            let (url, server) = serve(
                2,
                HashMap::from([
                    (
                        "/mods.json".into(),
                        (
                            manifest_bytes_with_size(&[("mod.jar", expected, declared_size)]),
                            None,
                        ),
                    ),
                    (
                        "/minecraft/mods/mod.jar".into(),
                        (body.to_vec(), Some(declared_size)),
                    ),
                ]),
            )
            .await;
            assert!(run_sync(&url, &game_dir, &managed_file).await.is_err());
            server.abort();
            assert_eq!(
                std::fs::read(game_dir.join("mods/mod.jar")).unwrap(),
                b"old"
            );
            assert_eq!(
                std::fs::read_dir(game_dir.join("mods"))
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
                    .count(),
                0
            );
        }
    }

    fn manifest_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        manifest_bytes_with_size(
            &files
                .iter()
                .map(|(name, bytes)| (*name, *bytes, bytes.len() as u64))
                .collect::<Vec<_>>(),
        )
    }

    fn manifest_bytes_with_size(files: &[(&str, &[u8], u64)]) -> Vec<u8> {
        serde_json::json!({
            "mods": files.iter().map(|(name, bytes, size)| {
                serde_json::json!({
                    "name": name,
                    "size": size,
                    "sha256": hex::encode(Sha256::digest(bytes)),
                    "path": format!("minecraft/mods/{name}"),
                })
            }).collect::<Vec<_>>()
        })
        .to_string()
        .into_bytes()
    }

    async fn run_sync(url: &Url, game_dir: &Path, managed_file: &Path) -> Result<(), String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|error| error.to_string())?;
        super::synchronize_from_url(&client, url, game_dir, managed_file, |_| Ok(())).await
    }

    async fn serve(
        connections: usize,
        routes: HashMap<String, (Vec<u8>, Option<u64>)>,
    ) -> (Url, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for _ in 0..connections {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut request = Vec::new();
                let mut chunk = [0u8; 512];
                loop {
                    let count = socket.read(&mut chunk).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/");
                let (status, body, declared_size) = match routes.get(path) {
                    Some((body, declared_size)) => ("200 OK", body.as_slice(), *declared_size),
                    None => ("404 Not Found", b"missing".as_slice(), None),
                };
                let length = declared_size.unwrap_or(body.len() as u64);
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
                );
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(body).await.unwrap();
            }
        });
        (
            Url::parse(&format!("http://{address}/mods.json")).unwrap(),
            task,
        )
    }
}
