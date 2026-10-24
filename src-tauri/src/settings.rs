use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub nickname: Option<String>,
    pub install_dir: Option<PathBuf>,
}

pub fn settings_directory(app: &AppHandle) -> Result<PathBuf, String> {
    let application_data = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Не удалось определить папку данных приложения: {error}"))?;
    settings_directory_from_app_specific_data(&application_data)
}

pub fn settings_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(settings_directory(app)?.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Result<Settings, String> {
    let path = settings_file(app)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => {
            return Err(format!(
                "Не удалось прочитать настройки {}: {error}",
                path.display()
            ))
        }
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Файл настроек {} повреждён или имеет неверный формат: {error}",
            path.display()
        )
    })
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_file(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Для файла настроек не определена родительская папка".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Не удалось создать папку настроек: {error}"))?;

    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("Не удалось сериализовать настройки: {error}"))?;
    atomic_write(&path, &bytes)
        .map_err(|error| format!("Не удалось сохранить настройки {}: {error}", path.display()))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{stamp}", std::process::id()));
    let mut file = OpenOptions::new()
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

fn settings_directory_from_app_specific_data(application_data: &Path) -> Result<PathBuf, String> {
    let roaming_data = application_data.parent().ok_or_else(|| {
        "Tauri вернул путь данных приложения без родительской папки APPDATA".to_string()
    })?;
    Ok(roaming_data.join("1stMay"))
}

#[cfg(test)]
mod tests {
    use super::settings_directory_from_app_specific_data;
    use std::path::Path;

    #[test]
    fn settings_directory_is_the_required_appdata_subdirectory() {
        assert_eq!(
            settings_directory_from_app_specific_data(Path::new(
                r"C:\Users\test\AppData\Roaming\com.1stmay.launcher",
            ))
            .unwrap(),
            Path::new(r"C:\Users\test\AppData\Roaming\1stMay")
        );
    }
}
