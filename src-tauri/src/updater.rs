use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const UPDATE_DIRECTORY: &str = "1stMay-updates";
const UPDATE_PACKAGE: &str = "launcher-update.exe";
const UPDATE_METADATA: &str = "update.json";

pub struct PendingUpdate(Mutex<Option<Update>>);

impl Default for PendingUpdate {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct CachedUpdate {
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    version: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    phase: &'static str,
    version: String,
    downloaded: usize,
    total: Option<u64>,
}

#[tauri::command]
pub async fn check_and_download_update(app: AppHandle) -> Result<Option<AvailableUpdate>, String> {
    let update_dir = update_directory()?;
    cleanup_completed_update(&update_dir)?;

    let updater = app
        .updater()
        .map_err(|error| format!("Не удалось подготовить проверку обновлений: {error}"))?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|error| format!("Не удалось проверить обновления: {error}"))?
    else {
        *app.state::<PendingUpdate>()
            .0
            .lock()
            .map_err(|_| "Состояние обновления повреждено".to_string())? = None;
        clear_update_files(&update_dir)?;
        return Ok(None);
    };
    if update.download_url.scheme() != "https" {
        return Err("Сервер обновлений должен предоставлять файлы по HTTPS".into());
    }

    let package_path = update_dir.join(UPDATE_PACKAGE);
    let metadata_path = update_dir.join(UPDATE_METADATA);
    let cached = read_cached_update(&metadata_path);
    if cached
        .as_ref()
        .is_some_and(|cached| cached.version == update.version)
    {
        if let Ok(bytes) = fs::read(&package_path) {
            if update.verify(&bytes).is_ok() {
                *app.state::<PendingUpdate>()
                    .0
                    .lock()
                    .map_err(|_| "Состояние обновления повреждено".to_string())? =
                    Some(update.clone());
                return Ok(Some(AvailableUpdate {
                    version: update.version,
                }));
            }
        }
    }

    fs::create_dir_all(&update_dir).map_err(|error| {
        format!(
            "Не удалось создать папку для обновления {}: {error}",
            update_dir.display()
        )
    })?;
    let app_for_progress = app.clone();
    let progress_version = update.version.clone();
    let mut downloaded = 0usize;
    let bytes = update
        .download(
            move |chunk_size, total| {
                downloaded += chunk_size;
                let _ = app_for_progress.emit(
                    "launcher-update-progress",
                    UpdateProgress {
                        phase: "downloading",
                        version: progress_version.clone(),
                        downloaded,
                        total,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(|error| format!("Не удалось скачать или проверить обновление: {error}"))?;

    write_atomic(&package_path, &bytes).map_err(|error| {
        format!(
            "Не удалось сохранить пакет обновления в {}: {error}",
            package_path.display()
        )
    })?;
    let metadata = serde_json::to_vec(&CachedUpdate {
        version: update.version.clone(),
    })
    .map_err(|error| format!("Не удалось подготовить сведения об обновлении: {error}"))?;
    write_atomic(&metadata_path, &metadata).map_err(|error| {
        format!(
            "Не удалось сохранить сведения об обновлении в {}: {error}",
            metadata_path.display()
        )
    })?;
    *app.state::<PendingUpdate>()
        .0
        .lock()
        .map_err(|_| "Состояние обновления повреждено".to_string())? = Some(update.clone());

    Ok(Some(AvailableUpdate {
        version: update.version,
    }))
}

#[tauri::command]
pub async fn install_downloaded_update(app: AppHandle) -> Result<(), String> {
    let update_dir = update_directory()?;
    let metadata_path = update_dir.join(UPDATE_METADATA);
    let cached = read_cached_update(&metadata_path)
        .ok_or_else(|| "Скачанный пакет обновления не найден. Повторите проверку.".to_string())?;
    let package_path = update_dir.join(UPDATE_PACKAGE);
    let bytes = fs::read(&package_path).map_err(|error| {
        format!(
            "Не удалось прочитать скачанный пакет {}: {error}",
            package_path.display()
        )
    })?;

    let pending = app
        .state::<PendingUpdate>()
        .0
        .lock()
        .map_err(|_| "Состояние обновления повреждено".to_string())?
        .clone();
    let update = match pending.filter(|update| update.version == cached.version) {
        Some(update) => update,
        None => {
            let updater = app
                .updater()
                .map_err(|error| format!("Не удалось подготовить установку обновления: {error}"))?;
            updater
                .check()
                .await
                .map_err(|error| format!("Не удалось повторно проверить обновление: {error}"))?
                .ok_or_else(|| "Обновление больше не доступно. Повторите проверку.".to_string())?
        }
    };
    if update.version != cached.version {
        return Err("Версия скачанного пакета устарела. Повторите проверку обновлений.".into());
    }

    update
        .install(&bytes)
        .map_err(|error| format!("Не удалось установить обновление: {error}"))
}

fn update_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Не удалось определить папку установленного лаунчера: {error}"))?;
    let parent = executable
        .parent()
        .ok_or_else(|| "Для исполняемого файла не найдена папка установки".to_string())?;
    Ok(parent.join(UPDATE_DIRECTORY))
}

fn read_cached_update(path: &Path) -> Option<CachedUpdate> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn cleanup_completed_update(update_dir: &Path) -> Result<(), String> {
    let Some(cached) = read_cached_update(&update_dir.join(UPDATE_METADATA)) else {
        return Ok(());
    };
    let installed = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("Некорректная версия лаунчера: {error}"))?;
    let downloaded = semver::Version::parse(&cached.version)
        .map_err(|error| format!("Некорректная версия скачанного обновления: {error}"))?;
    if downloaded <= installed {
        clear_update_files(update_dir)?;
    }
    Ok(())
}

fn clear_update_files(update_dir: &Path) -> Result<(), String> {
    for name in [UPDATE_PACKAGE, UPDATE_METADATA] {
        let path = update_dir.join(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Не удалось удалить файл обновления {}: {error}",
                    path.display()
                ))
            }
        }
    }
    if update_dir.is_dir() {
        match fs::remove_dir(update_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Не удалось удалить пустую папку обновлений {}: {error}",
                    update_dir.display()
                ))
            }
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{timestamp}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cleanup_removes_only_the_managed_update_files() {
        let directory = tempdir().unwrap();
        let updates = directory.path().join(UPDATE_DIRECTORY);
        fs::create_dir(&updates).unwrap();
        fs::write(updates.join(UPDATE_PACKAGE), b"installer").unwrap();
        fs::write(updates.join(UPDATE_METADATA), br#"{"version":"1.0.0"}"#).unwrap();
        fs::write(updates.join("keep.txt"), b"user file").unwrap();

        clear_update_files(&updates).unwrap();

        assert_eq!(fs::read(updates.join("keep.txt")).unwrap(), b"user file");
        assert!(updates.is_dir());
        assert!(!updates.join(UPDATE_PACKAGE).exists());
    }

    #[test]
    fn cleanup_removes_a_package_after_its_version_is_installed() {
        let directory = tempdir().unwrap();
        let updates = directory.path().join(UPDATE_DIRECTORY);
        fs::create_dir(&updates).unwrap();
        let installed = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        fs::write(updates.join(UPDATE_PACKAGE), b"installer").unwrap();
        fs::write(
            updates.join(UPDATE_METADATA),
            format!(r#"{{"version":"{installed}"}}"#),
        )
        .unwrap();

        cleanup_completed_update(&updates).unwrap();

        assert!(!updates.join(UPDATE_METADATA).exists());
        assert!(!updates.join(UPDATE_PACKAGE).exists());
    }
}
