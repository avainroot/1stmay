use crate::{commands, settings};
use base64::{engine::general_purpose::STANDARD, Engine};
use png::Decoder;
use serde::Serialize;
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Emitter};

const MAX_SKIN_BYTES: usize = 2 * 1024 * 1024;
const SKIN_CATEGORIES: [&str; 3] = ["skins", "capes", "elytras"];
static SYNCHRONIZING: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinPreview {
    data_url: String,
}

#[derive(Clone, Serialize)]
struct Progress {
    message: &'static str,
}

#[tauri::command]
pub fn get_current_skin(app: AppHandle) -> Result<Option<SkinPreview>, String> {
    let Some(nickname) = settings::load(&app)?.nickname else {
        return Ok(None);
    };
    commands::validate_nickname(&nickname)?;
    let path = stored_skin_path(&settings::settings_directory(&app)?, &nickname);
    match fs::read(&path) {
        Ok(bytes) => preview_from_bytes(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Не удалось прочитать сохранённый скин: {error}")),
    }
}

#[tauri::command]
pub fn load_skin_preview(path: String) -> Result<SkinPreview, String> {
    let bytes = read_valid_skin(Path::new(&path))?;
    preview_from_bytes(&bytes)
}

#[tauri::command]
pub fn save_skin(app: AppHandle, path: String) -> Result<(), String> {
    let nickname = current_nickname(&app)?;
    let bytes = read_valid_skin(Path::new(&path))?;
    let root = settings::settings_directory(&app)?.join("LocalSkin");
    save_skin_file(&root, &nickname, &bytes)
}

#[tauri::command]
pub fn delete_skin(app: AppHandle) -> Result<(), String> {
    let nickname = current_nickname(&app)?;
    let root = settings::settings_directory(&app)?.join("LocalSkin");
    let path = skin_path(&root, "skins", &nickname);
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Не удалось удалить скин для {nickname}: {error}")),
    }
}

pub fn synchronize_before_launch(app: &AppHandle, game_dir: &Path) -> Result<(), String> {
    if SYNCHRONIZING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Err("Синхронизация локального скина уже выполняется.".into());
    }
    let _guard = SyncGuard;
    let nickname = current_nickname(app)?;
    app.emit(
        "launcher-progress",
        Progress {
            message: "Подготовка локального скина…",
        },
    )
    .map_err(|error| format!("Не удалось обновить статус подготовки скина: {error}"))?;

    let root = settings::settings_directory(app)?.join("LocalSkin");
    synchronize_player_skin(&root, game_dir, &nickname)
}

fn synchronize_player_skin(
    storage_root: &Path,
    game_dir: &Path,
    nickname: &str,
) -> Result<(), String> {
    commands::validate_nickname(nickname)?;
    ensure_categories(storage_root)?;

    let loader_root = game_dir.join("CustomSkinLoader").join("LocalSkin");
    ensure_categories(&loader_root)?;

    let source = skin_path(storage_root, "skins", nickname);
    let destination = skin_path(&loader_root, "skins", nickname);
    match fs::read(&source) {
        Ok(bytes) => {
            validate_skin_png(&bytes)?;
            settings::atomic_write(&destination, &bytes)
                .map_err(|error| format!("Не удалось подготовить скин для Minecraft: {error}"))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::remove_file(destination) {
                Ok(()) => {}
                Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {}
                Err(remove_error) => {
                    return Err(format!(
                        "Не удалось удалить устаревшую копию скина: {remove_error}"
                    ))
                }
            }
        }
        Err(error) => return Err(format!("Не удалось прочитать локальный скин: {error}")),
    }
    Ok(())
}

fn current_nickname(app: &AppHandle) -> Result<String, String> {
    let nickname = settings::load(app)?
        .nickname
        .ok_or_else(|| "Сначала сохраните никнейм Minecraft.".to_string())?;
    commands::validate_nickname(&nickname)?;
    Ok(nickname)
}

fn ensure_categories(root: &Path) -> Result<(), String> {
    for category in SKIN_CATEGORIES {
        fs::create_dir_all(root.join(category))
            .map_err(|error| format!("Не удалось подготовить папку текстур {category}: {error}"))?;
    }
    Ok(())
}

fn skin_path(root: &Path, category: &str, nickname: &str) -> PathBuf {
    root.join(category).join(format!("{nickname}.png"))
}

fn stored_skin_path(settings_directory: &Path, nickname: &str) -> PathBuf {
    skin_path(&settings_directory.join("LocalSkin"), "skins", nickname)
}

fn save_skin_file(root: &Path, nickname: &str, bytes: &[u8]) -> Result<(), String> {
    commands::validate_nickname(nickname)?;
    validate_skin_png(bytes)?;
    ensure_categories(root)?;
    let destination = skin_path(root, "skins", nickname);
    settings::atomic_write(&destination, bytes)
        .map_err(|error| format!("Не удалось сохранить скин для {nickname}: {error}"))
}

fn read_valid_skin(path: &Path) -> Result<Vec<u8>, String> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        return Err("Выберите файл PNG.".into());
    }
    let file_size = fs::metadata(path)
        .map_err(|error| format!("Не удалось прочитать сведения о PNG-файле: {error}"))?
        .len();
    if file_size > MAX_SKIN_BYTES as u64 {
        return Err("PNG скина превышает допустимый размер 2 МиБ.".into());
    }
    let bytes =
        fs::read(path).map_err(|error| format!("Не удалось прочитать PNG-файл: {error}"))?;
    validate_skin_png(&bytes)?;
    Ok(bytes)
}

fn validate_skin_png(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_SKIN_BYTES {
        return Err("PNG скина превышает допустимый размер 2 МиБ.".into());
    }
    let mut reader = Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(|error| format!("Файл не является корректным PNG: {error}"))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if !matches!((width, height), (64, 64) | (64, 32)) {
        return Err(format!(
            "Размер текстуры {width}×{height} не поддерживается. Используйте PNG 64×64 или 64×32."
        ));
    }
    let output_size = reader.output_buffer_size().ok_or_else(|| {
        "Размер декодированных данных PNG превышает допустимый предел.".to_string()
    })?;
    let mut decoded = vec![0; output_size];
    reader
        .next_frame(&mut decoded)
        .map_err(|error| format!("Не удалось декодировать PNG скина: {error}"))?;
    Ok(())
}

fn preview_from_bytes(bytes: &[u8]) -> Result<SkinPreview, String> {
    validate_skin_png(bytes)?;
    Ok(SkinPreview {
        data_url: format!("data:image/png;base64,{}", STANDARD.encode(bytes)),
    })
}

struct SyncGuard;

impl Drop for SyncGuard {
    fn drop(&mut self) {
        SYNCHRONIZING.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        save_skin_file, stored_skin_path, synchronize_player_skin, validate_skin_png,
        MAX_SKIN_BYTES,
    };
    use png::{BitDepth, ColorType, Encoder};
    use std::fs;
    use tempfile::tempdir;

    fn png_bytes(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = Encoder::new(&mut bytes, width, height);
            encoder.set_color(ColorType::Rgba);
            encoder.set_depth(BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&color.repeat((width * height) as usize))
                .unwrap();
        }
        bytes
    }

    #[test]
    fn accepts_standard_and_legacy_minecraft_skin_dimensions() {
        assert!(validate_skin_png(&png_bytes(64, 64, [10, 20, 30, 255])).is_ok());
        assert!(validate_skin_png(&png_bytes(64, 32, [10, 20, 30, 255])).is_ok());
    }

    #[test]
    fn rejects_invalid_dimensions_and_oversized_pngs() {
        assert!(validate_skin_png(&png_bytes(128, 128, [0, 0, 0, 255])).is_err());
        assert!(validate_skin_png(&vec![0; MAX_SKIN_BYTES + 1]).is_err());
    }

    #[test]
    fn resolves_saved_skin_inside_local_skin_storage() {
        let settings_directory = std::path::Path::new("settings");
        assert_eq!(
            stored_skin_path(settings_directory, "Player1"),
            settings_directory
                .join("LocalSkin")
                .join("skins")
                .join("Player1.png")
        );
    }

    #[test]
    fn copies_and_removes_only_the_current_player_skin() {
        let temporary = tempdir().unwrap();
        let storage = temporary.path().join("LocalSkin");
        let game = temporary.path().join("game");
        let source = storage.join("skins").join("Player1.png");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let original = png_bytes(64, 64, [10, 20, 30, 255]);
        fs::write(&source, &original).unwrap();
        let loader_root = game.join("CustomSkinLoader");
        fs::create_dir_all(&loader_root).unwrap();
        fs::write(loader_root.join("CustomSkinLoader.json"), b"unrelated").unwrap();
        let unrelated_skin_dir = loader_root.join("LocalSkin/skins");
        fs::create_dir_all(&unrelated_skin_dir).unwrap();
        fs::write(unrelated_skin_dir.join("OtherPlayer.png"), b"unrelated").unwrap();

        synchronize_player_skin(&storage, &game, "Player1").unwrap();
        let copied = unrelated_skin_dir.join("Player1.png");
        assert_eq!(fs::read(&copied).unwrap(), original);

        let replacement = png_bytes(64, 64, [40, 50, 60, 255]);
        fs::write(&source, &replacement).unwrap();
        synchronize_player_skin(&storage, &game, "Player1").unwrap();
        assert_eq!(fs::read(&copied).unwrap(), replacement);

        fs::remove_file(source).unwrap();
        synchronize_player_skin(&storage, &game, "Player1").unwrap();
        assert!(!copied.exists());
        assert_eq!(
            fs::read(loader_root.join("CustomSkinLoader.json")).unwrap(),
            b"unrelated"
        );
        assert_eq!(
            fs::read(unrelated_skin_dir.join("OtherPlayer.png")).unwrap(),
            b"unrelated"
        );
    }

    #[test]
    fn rejects_path_like_player_names_before_building_skin_path() {
        assert!(crate::commands::validate_nickname("../Player").is_err());
    }

    #[test]
    fn rejects_corrupt_png_data() {
        assert!(validate_skin_png(b"not a PNG").is_err());
    }

    #[test]
    fn failed_skin_replacement_preserves_the_previous_saved_texture() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("LocalSkin");
        let original = png_bytes(64, 64, [10, 20, 30, 255]);
        save_skin_file(&root, "Player1", &original).unwrap();
        assert!(save_skin_file(&root, "Player1", b"invalid replacement").is_err());
        assert_eq!(fs::read(root.join("skins/Player1.png")).unwrap(), original);

        let replacement = png_bytes(64, 64, [40, 50, 60, 255]);
        save_skin_file(&root, "Player1", &replacement).unwrap();
        assert_eq!(
            fs::read(root.join("skins/Player1.png")).unwrap(),
            replacement
        );
    }
}
