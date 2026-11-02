use crate::{
    config::{MINECRAFT_VERSION, NEOFORGE_VERSION},
    launch, memory, minecraft, platform, settings, sync,
};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherState {
    nickname: Option<String>,
    install_dir: Option<String>,
    installed: bool,
    drive_error: Option<String>,
    config_synced: bool,
}

#[tauri::command]
pub fn get_launcher_state(app: AppHandle) -> Result<LauncherState, String> {
    let stored = settings::load(&app)?;
    let detected = if stored.install_dir.is_none() {
        platform::default_install_directory()
    } else {
        Ok(PathBuf::new())
    };
    let install_dir = stored
        .install_dir
        .clone()
        .or_else(|| detected.as_ref().ok().cloned());
    let installed = install_dir
        .as_ref()
        .is_some_and(|path| minecraft::is_installed(path));

    Ok(LauncherState {
        nickname: stored.nickname,
        install_dir: install_dir.map(|path| path.to_string_lossy().into_owned()),
        installed,
        drive_error: detected.err(),
        config_synced: stored.config_synced,
    })
}

#[tauri::command]
pub fn get_game_status() -> bool {
    launch::is_game_running()
}

#[tauri::command]
pub fn get_memory_status() -> Result<memory::MemoryStatus, String> {
    memory::memory_status()
}

#[tauri::command]
pub fn save_install_directory(app: AppHandle, path: String) -> Result<(), String> {
    let selected = PathBuf::from(path);
    if !selected.is_absolute() {
        return Err("Путь установки должен быть абсолютным".into());
    }
    if !selected.is_dir() {
        return Err("Выбранная папка установки не существует или недоступна".into());
    }
    let mut stored = settings::load(&app)?;
    if stored
        .install_dir
        .as_deref()
        .is_some_and(minecraft::is_installed)
    {
        return Err("Каталог установки нельзя менять после установки игры".into());
    }
    stored.install_dir = Some(selected);
    settings::save(&app, &stored)
}

#[tauri::command]
pub async fn play(app: AppHandle, nickname: Option<String>) -> Result<bool, String> {
    let mut stored = settings::load(&app)?;
    let nickname = match stored.nickname.as_ref() {
        Some(saved) => saved.clone(),
        None => nickname.ok_or_else(|| "Введите никнейм перед запуском".to_string())?,
    };
    validate_nickname(&nickname)?;
    if stored.nickname.is_none() {
        stored.nickname = Some(nickname.clone());
        settings::save(&app, &stored)?;
    }

    let install_dir = match stored.install_dir.clone() {
        Some(path) => path,
        None => platform::default_install_directory()?,
    };
    if stored.install_dir.is_none() {
        stored.install_dir = Some(install_dir.clone());
        settings::save(&app, &stored)?;
    }

    app.emit(
        "launcher-progress",
        Progress {
            message: format!(
                "Проверка установки Minecraft {MINECRAFT_VERSION} / NeoForge {NEOFORGE_VERSION}…"
            ),
        },
    )
    .map_err(|error| format!("Не удалось обновить статус лаунчера: {error}"))?;

    let java = minecraft::ensure_installed(&app, &install_dir).await?;
    if !minecraft::is_installed(&install_dir) {
        return Err("Проверка после установки Minecraft/NeoForge не пройдена".into());
    }
    memory::available_heap_mib()?;
    if stored.config_synced {
        sync::synchronize_mods(&app, &install_dir).await?;
    } else {
        sync::synchronize_mods_and_configs(&app, &install_dir).await?;
        stored.config_synced = true;
        settings::save(&app, &stored)?;
    }
    launch::launch(&app, &install_dir, &nickname, &java).await
}

#[tauri::command]
pub async fn synchronize_config_files(app: AppHandle) -> Result<(), String> {
    if launch::is_game_running() {
        return Err("Закройте Minecraft перед синхронизацией конфигураций.".into());
    }
    let mut stored = settings::load(&app)?;
    let install_dir = stored
        .install_dir
        .clone()
        .ok_or_else(|| "Папка игры не настроена. Сначала установите Minecraft.".to_string())?;
    if !minecraft::is_installed(&install_dir) {
        return Err("Сначала установите Minecraft, затем синхронизируйте конфигурации.".into());
    }

    sync::synchronize_configs(&app, &install_dir).await?;
    stored.config_synced = true;
    settings::save(&app, &stored)
}

#[derive(Clone, Serialize)]
struct Progress {
    message: String,
}

pub fn validate_nickname(nickname: &str) -> Result<(), String> {
    let valid = (3..=16).contains(&nickname.len())
        && nickname
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(
            "Никнейм должен содержать 3–16 символов: латинские буквы, цифры или подчёркивание."
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::validate_nickname;

    #[test]
    fn accepts_valid_minecraft_nicknames() {
        assert!(validate_nickname("Player_123").is_ok());
    }

    #[test]
    fn rejects_invalid_nicknames_before_work_starts() {
        for nickname in ["ab", "a name", "кириллица", "name!", "abcdefghijklmnopq"] {
            assert!(validate_nickname(nickname).is_err(), "{nickname}");
        }
    }
}
