use crate::{
    config,
    metadata::{self, VersionMetadata},
    runtime, settings,
};
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tokio::process::Command;

pub async fn ensure_installed(app: &AppHandle, game_dir: &Path) -> Result<PathBuf, String> {
    if is_installed(game_dir) {
        let java = runtime::ensure_java(app).await?;
        return Ok(java);
    }

    metadata::install_vanilla(app, game_dir).await?;
    let java = runtime::ensure_java(app).await?;
    install_neoforge(app, game_dir, &java).await?;
    if !is_installed(game_dir) {
        return Err(
            "Установка завершилась, но профиль NeoForge или клиент Minecraft не найден".into(),
        );
    }
    Ok(java)
}

pub fn is_installed(game_dir: &Path) -> bool {
    let profile_path = profile_path(game_dir);
    let vanilla_metadata_path = game_dir
        .join("versions")
        .join(config::MINECRAFT_VERSION)
        .join(format!("{}.json", config::MINECRAFT_VERSION));
    let client = game_dir
        .join("versions")
        .join(config::MINECRAFT_VERSION)
        .join(format!("{}.jar", config::MINECRAFT_VERSION));
    if !profile_path.is_file() || !client.is_file() || !vanilla_metadata_path.is_file() {
        return false;
    }
    let Ok(profile) = fs::read_to_string(profile_path) else {
        return false;
    };
    let Ok(profile): Result<Value, _> = serde_json::from_str(&profile) else {
        return false;
    };
    let Ok(vanilla_bytes) = fs::read(vanilla_metadata_path) else {
        return false;
    };
    let Ok(vanilla): Result<VersionMetadata, _> = serde_json::from_slice(&vanilla_bytes) else {
        return false;
    };
    let Some(client_artifact) = vanilla
        .downloads
        .as_ref()
        .map(|downloads| &downloads.client)
    else {
        return false;
    };
    metadata::artifact_is_valid(&client, client_artifact)
        && is_neoforge_universal_valid(game_dir)
        && profile.get("id").and_then(Value::as_str) == Some(config::NEOFORGE_PROFILE_ID)
        && profile.get("inheritsFrom").and_then(Value::as_str) == Some(config::MINECRAFT_VERSION)
        && profile.get("mainClass").and_then(Value::as_str).is_some()
}

pub fn load_profile(game_dir: &Path) -> Result<VersionMetadata, String> {
    let path = profile_path(game_dir);
    let profile: VersionMetadata = serde_json::from_slice(
        &fs::read(&path)
            .map_err(|error| format!("Не удалось прочитать профиль NeoForge: {error}"))?,
    )
    .map_err(|error| format!("Профиль NeoForge повреждён: {error}"))?;
    if profile.id != config::NEOFORGE_PROFILE_ID
        || profile.inherits_from.as_deref() != Some(config::MINECRAFT_VERSION)
    {
        return Err("Профиль NeoForge не соответствует зафиксированной сборке".into());
    }
    Ok(profile)
}

fn profile_path(game_dir: &Path) -> PathBuf {
    game_dir
        .join("versions")
        .join(config::NEOFORGE_PROFILE_ID)
        .join(format!("{}.json", config::NEOFORGE_PROFILE_ID))
}

async fn install_neoforge(app: &AppHandle, game_dir: &Path, java: &Path) -> Result<(), String> {
    emit_progress(
        app,
        &format!("Подготовка NeoForge {}…", config::NEOFORGE_VERSION),
    )?;
    let app_dir = settings::settings_directory(app)?;
    let cache = app_dir.join("cache");
    fs::create_dir_all(&cache)
        .map_err(|error| format!("Не удалось создать кэш установщика: {error}"))?;
    let installer = cache.join(format!(
        "neoforge-{}-installer.jar",
        config::NEOFORGE_VERSION
    ));
    ensure_installer(app, &installer).await?;
    ensure_neoforge_universal(app, game_dir).await?;

    let launcher_profiles = game_dir.join("launcher_profiles.json");
    if !launcher_profiles.exists() {
        fs::create_dir_all(game_dir)
            .map_err(|error| format!("Не удалось создать папку игры: {error}"))?;
        write_new_file(&launcher_profiles, br#"{"profiles":{},"settings":{}}"#)
            .map_err(|error| format!("Не удалось подготовить NeoForge к установке: {error}"))?;
    }

    emit_progress(app, "Установка загрузчика NeoForge…")?;
    let output = Command::new(java)
        .arg("-jar")
        .arg(&installer)
        .arg("--install-client")
        .arg(game_dir)
        .current_dir(&cache)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| {
            format!("Не удалось запустить официальный установщик NeoForge: {error}")
        })?;
    if !output.status.success() {
        let detail = installer_failure_detail(&output.stdout, &output.stderr);
        return Err(format!(
            "Установщик NeoForge завершился с кодом {}.\n{}",
            output.status, detail
        ));
    }

    let profile = load_profile(game_dir)?;
    if profile.main_class.trim().is_empty() {
        return Err("Установщик NeoForge создал профиль без mainClass".into());
    }
    Ok(())
}

pub fn neoforge_universal_path(game_dir: &Path) -> PathBuf {
    game_dir
        .join("libraries")
        .join(config::NEOFORGE_UNIVERSAL_PATH)
}

pub fn is_neoforge_universal_valid(game_dir: &Path) -> bool {
    fs::read(neoforge_universal_path(game_dir))
        .ok()
        .is_some_and(|bytes| verify_neoforge_universal(&bytes).is_ok())
}

async fn ensure_neoforge_universal(app: &AppHandle, game_dir: &Path) -> Result<(), String> {
    let destination = neoforge_universal_path(game_dir);
    if is_neoforge_universal_valid(game_dir) {
        return Ok(());
    }

    emit_progress(app, "Подготовка основного модуля NeoForge…")?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|error| format!("Не удалось создать HTTP-клиент NeoForge: {error}"))?;

    let bytes = match fetch_verified_neoforge_universal(&client, config::NEOFORGE_UNIVERSAL_URL)
        .await
    {
        Ok(bytes) => bytes,
        Err(official_error) => {
            let fallback_url = neoforge_universal_fallback_url()?;
            emit_progress(
                app,
                "Официальный источник NeoForge недоступен; переключение на резервный источник…",
            )?;
            fetch_verified_neoforge_universal(&client, &fallback_url)
                .await
                .map_err(|fallback_error| {
                    format!(
                        "Не удалось получить обязательный модуль NeoForge. Официальный источник: {official_error}. Резервный источник: {fallback_error}. Проверьте доступ к источникам и наличие резервного файла universal."
                    )
                })?
        }
    };

    write_atomic(&destination, &bytes).map_err(|error| {
        format!(
            "Не удалось сохранить основной модуль NeoForge {}: {error}",
            destination.display()
        )
    })
}

async fn fetch_verified_neoforge_universal(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<u8>, String> {
    let bytes = metadata::fetch_bytes(client, url).await?;
    verify_neoforge_universal(&bytes)?;
    Ok(bytes)
}

fn verify_neoforge_universal(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != config::NEOFORGE_UNIVERSAL_SIZE {
        return Err(format!(
            "размер universal-модуля NeoForge не совпал: ожидалось {} байт, получено {}",
            config::NEOFORGE_UNIVERSAL_SIZE,
            bytes.len()
        ));
    }
    let actual = hex::encode(Sha1::digest(bytes));
    if actual != config::NEOFORGE_UNIVERSAL_SHA1 {
        return Err(format!(
            "SHA-1 universal-модуля NeoForge не совпал: ожидалось {}, получено {actual}",
            config::NEOFORGE_UNIVERSAL_SHA1
        ));
    }
    Ok(())
}

fn neoforge_universal_fallback_url() -> Result<String, String> {
    let mut url = reqwest::Url::parse(config::NEOFORGE_FALLBACK_URL)
        .map_err(|error| format!("Резервный URL NeoForge имеет неверный формат: {error}"))?;
    if url.scheme() != "https" {
        return Err("Резервный источник NeoForge должен использовать HTTPS".into());
    }
    let filename = url
        .path_segments()
        .and_then(Iterator::last)
        .ok_or_else(|| "В резервном URL NeoForge не задано имя файла".to_string())?;
    let universal_filename = filename
        .strip_suffix("-installer.jar")
        .map(|stem| format!("{stem}-universal.jar"))
        .ok_or_else(|| {
            "Резервный URL установщика NeoForge должен оканчиваться на -installer.jar".to_string()
        })?;
    url.path_segments_mut()
        .map_err(|()| "Не удалось подготовить резервный URL NeoForge".to_string())?
        .pop()
        .push(&universal_filename);
    Ok(url.into())
}

async fn ensure_installer(app: &AppHandle, destination: &Path) -> Result<(), String> {
    if destination.is_file() {
        let existing = fs::read(destination)
            .map_err(|error| format!("Не удалось проверить установщик NeoForge: {error}"))?;
        if sha256(&existing) == config::NEOFORGE_INSTALLER_SHA256 {
            return Ok(());
        }
    }
    let official_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|error| format!("Не удалось создать HTTP-клиент NeoForge: {error}"))?;
    emit_progress(
        app,
        "Загрузка установщика NeoForge с официального источника…",
    )?;
    let bytes = match fetch_neoforge_installer(&official_client, config::NEOFORGE_INSTALLER_URL)
        .await
    {
        Ok(bytes) => bytes,
        Err(InstallerDownloadError::Timeout) => {
            emit_progress(
                app,
                "Официальный источник NeoForge отвечает дольше 8 секунд; переключение на резервный источник…",
            )?;
            emit_progress(app, "Загрузка установщика NeoForge с резервного источника…")?;
            let fallback_client = metadata::http_client()?;
            fetch_neoforge_installer(&fallback_client, config::NEOFORGE_FALLBACK_URL)
                .await
                .map_err(|error| {
                    format!(
                        "Истекло ожидание официального источника NeoForge; резервная загрузка не удалась: {}",
                        error.message()
                    )
                })?
        }
        Err(InstallerDownloadError::Failed(error)) => {
            return Err(format!(
                "Не удалось скачать официальный установщик NeoForge: {error}"
            ))
        }
    };
    let actual = sha256(&bytes);
    if actual != config::NEOFORGE_INSTALLER_SHA256 {
        return Err(format!(
            "SHA-256 загруженного установщика NeoForge не совпал: ожидалось {}, получено {actual}",
            config::NEOFORGE_INSTALLER_SHA256
        ));
    }
    write_atomic(destination, &bytes)
        .map_err(|error| format!("Не удалось сохранить установщик NeoForge: {error}"))
}

enum InstallerDownloadError {
    Timeout,
    Failed(String),
}

impl InstallerDownloadError {
    fn message(self) -> String {
        match self {
            Self::Timeout => "превышено время ожидания".into(),
            Self::Failed(message) => message,
        }
    }
}

async fn fetch_neoforge_installer(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<u8>, InstallerDownloadError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| InstallerDownloadError::Failed(format!("неверный URL: {error}")))?;
    if parsed.scheme() != "https" {
        return Err(InstallerDownloadError::Failed(
            "URL установщика должен использовать HTTPS".into(),
        ));
    }
    let response = client.get(parsed).send().await.map_err(|error| {
        if error.is_timeout() {
            InstallerDownloadError::Timeout
        } else {
            InstallerDownloadError::Failed(error.to_string())
        }
    })?;
    if response.url().scheme() != "https" {
        return Err(InstallerDownloadError::Failed(
            "Загрузка была перенаправлена на незашифрованный URL".into(),
        ));
    }
    let response = response.error_for_status().map_err(|error| {
        if error.is_timeout() {
            InstallerDownloadError::Timeout
        } else {
            InstallerDownloadError::Failed(error.to_string())
        }
    })?;
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| {
            if error.is_timeout() {
                InstallerDownloadError::Timeout
            } else {
                InstallerDownloadError::Failed(error.to_string())
            }
        })
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn installer_failure_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!(
            "stderr:\n{}\nstdout:\n{}",
            truncate(stderr, 2000),
            truncate(stdout, 2000)
        ),
        (false, true) => format!("stdout:\n{}", truncate(stdout, 4000)),
        (true, false) => format!("stderr:\n{}", truncate(stderr, 4000)),
        (true, true) => "Установщик не вывел диагностический текст.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        fetch_neoforge_installer, installer_failure_detail, neoforge_universal_fallback_url,
        verify_neoforge_universal, InstallerDownloadError,
    };
    use crate::config;

    #[test]
    fn installer_failure_detail_includes_stdout_when_stderr_is_empty() {
        assert_eq!(
            installer_failure_detail(b"Installer failed: missing library", b""),
            "stdout:\nInstaller failed: missing library"
        );
    }

    #[test]
    fn installer_failure_detail_reports_when_both_streams_are_empty() {
        assert_eq!(
            installer_failure_detail(b"", b""),
            "Установщик не вывел диагностический текст."
        );
    }

    #[tokio::test]
    async fn rejects_non_https_neoforge_sources_before_requesting_them() {
        let client = reqwest::Client::new();
        let error = fetch_neoforge_installer(&client, "http://localhost/installer.jar")
            .await
            .unwrap_err();
        assert!(matches!(&error, InstallerDownloadError::Failed(_)));
        assert!(error.message().contains("HTTPS"));
    }

    #[test]
    fn universal_artifact_rejects_wrong_size_before_hash_comparison() {
        let error = verify_neoforge_universal(b"not the NeoForge jar").unwrap_err();
        assert!(error.contains("размер"));
    }

    #[test]
    fn universal_artifact_rejects_a_hash_mismatch_at_the_expected_size() {
        let bytes = vec![0; config::NEOFORGE_UNIVERSAL_SIZE as usize];
        let error = verify_neoforge_universal(&bytes).unwrap_err();
        assert!(error.contains("SHA-1"));
    }

    #[test]
    fn universal_fallback_is_the_sibling_of_the_configured_installer() {
        let fallback = neoforge_universal_fallback_url().unwrap();
        let expected = config::NEOFORGE_FALLBACK_URL.replace("-installer.jar", "-universal.jar");
        assert_eq!(fallback, expected);
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "для установщика нет родительской папки",
        )
    })?;
    fs::create_dir_all(parent)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{stamp}", std::process::id()));
    write_new_file(&temporary, bytes)?;
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

fn truncate(value: &str, max_chars: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return value.to_string();
    }
    let tail = chars[chars.len() - max_chars..].iter().collect::<String>();
    format!("…{tail}")
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
