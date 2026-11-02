use crate::{
    config,
    metadata::{self, VersionMetadata},
    runtime, settings,
};
use serde_json::Value;
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
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Установщик NeoForge завершился с кодом {}: {}",
            output.status,
            truncate(&detail, 4000)
        ));
    }

    let profile = load_profile(game_dir)?;
    if profile.main_class.trim().is_empty() {
        return Err("Установщик NeoForge создал профиль без mainClass".into());
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{fetch_neoforge_installer, InstallerDownloadError};

    #[tokio::test]
    async fn rejects_non_https_neoforge_sources_before_requesting_them() {
        let client = reqwest::Client::new();
        let error = fetch_neoforge_installer(&client, "http://localhost/installer.jar")
            .await
            .unwrap_err();
        assert!(matches!(&error, InstallerDownloadError::Failed(_)));
        assert!(error.message().contains("HTTPS"));
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
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        truncated.push('…');
    }
    truncated
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
