use crate::{commands, config, settings};
use base64::{engine::general_purpose::STANDARD, Engine};
use png::Decoder;
use reqwest::{multipart, Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::{io::Cursor, path::Path, time::Duration};
use tauri::AppHandle;
use url::Url;

const MAX_SKIN_BYTES: usize = 2 * 1024 * 1024;
const SKIN_TYPE: &str = "skins";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinPreview {
    data_url: String,
}

#[derive(Deserialize)]
struct UploadResponse {
    success: bool,
    username: String,
    #[serde(rename = "type")]
    upload_type: String,
    filename: String,
}

#[tauri::command]
pub async fn get_current_skin(app: AppHandle) -> Result<Option<SkinPreview>, String> {
    let Some(nickname) = settings::load(&app)?.nickname else {
        return Ok(None);
    };
    commands::validate_nickname(&nickname)?;
    let (_, image_url) = texture_urls(config::TEXTURE_API_BASE_URL, &nickname)?;
    let response = http_client()?
        .get(image_url)
        .send()
        .await
        .map_err(|error| format!("Не удалось получить скин из API: {error}"))?;

    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }

    let response = response
        .error_for_status()
        .map_err(|error| format!("Не удалось получить скин из API: {error}"))?;
    let bytes = read_bounded_response(response).await?;
    preview_from_bytes(&bytes).map(Some)
}

#[tauri::command]
pub fn load_skin_preview(path: String) -> Result<SkinPreview, String> {
    let bytes = read_valid_skin(Path::new(&path))?;
    preview_from_bytes(&bytes)
}

#[tauri::command]
pub async fn save_skin(app: AppHandle, path: String) -> Result<(), String> {
    let nickname = current_nickname(&app)?;
    let bytes = read_valid_skin(Path::new(&path))?;
    let (upload_url, _) = texture_urls(config::TEXTURE_API_BASE_URL, &nickname)?;
    let file = multipart::Part::bytes(bytes)
        .file_name(format!("{nickname}.png"))
        .mime_str("image/png")
        .map_err(|error| format!("Не удалось подготовить PNG для загрузки: {error}"))?;
    let form = multipart::Form::new()
        .text("username", nickname.clone())
        .text("type", SKIN_TYPE)
        .part("file", file);

    let response = http_client()?
        .post(upload_url)
        .multipart(form)
        .send()
        .await
        .map_err(|error| format!("Не удалось загрузить скин в API: {error}"))?;
    let response = response
        .error_for_status()
        .map_err(|error| format!("API не сохранил скин: {error}"))?;
    let result: UploadResponse = response
        .json()
        .await
        .map_err(|error| format!("API вернул некорректный ответ о загрузке скина: {error}"))?;

    if !result.success
        || result.username != nickname
        || result.upload_type != SKIN_TYPE
        || result.filename != format!("{nickname}.png")
    {
        return Err("API вернул неожиданный результат загрузки скина.".into());
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

fn http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Не удалось подготовить подключение к API текстур: {error}"))
}

fn texture_urls(api_base: &str, nickname: &str) -> Result<(Url, Url), String> {
    commands::validate_nickname(nickname)?;
    let mut base =
        Url::parse(api_base).map_err(|error| format!("Некорректный адрес API текстур: {error}"))?;
    if base.scheme() != "https"
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err("Адрес API текстур должен быть безопасным HTTPS URL.".into());
    }

    let api_path = base.path().trim_end_matches('/');
    base.set_path(&format!("{api_path}/"));
    let upload_url = base
        .join("upload")
        .map_err(|error| format!("Некорректный адрес загрузки скинов: {error}"))?;
    let mut image_url = base;
    image_url.set_path(&format!(
        "/uploads/users/{nickname}/{SKIN_TYPE}/{nickname}.png"
    ));

    Ok((upload_url, image_url))
}

async fn read_bounded_response(mut response: Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SKIN_BYTES as u64)
    {
        return Err("Скин из API превышает допустимый размер 2 МиБ.".into());
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Не удалось прочитать скин из API: {error}"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_SKIN_BYTES {
            return Err("Скин из API превышает допустимый размер 2 МиБ.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn read_valid_skin(path: &Path) -> Result<Vec<u8>, String> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        return Err("Выберите файл PNG.".into());
    }
    let file_size = std::fs::metadata(path)
        .map_err(|error| format!("Не удалось прочитать сведения о PNG-файле: {error}"))?
        .len();
    if file_size > MAX_SKIN_BYTES as u64 {
        return Err("PNG скина превышает допустимый размер 2 МиБ.".into());
    }
    let bytes =
        std::fs::read(path).map_err(|error| format!("Не удалось прочитать PNG-файл: {error}"))?;
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

#[cfg(test)]
mod tests {
    use super::{texture_urls, validate_skin_png, MAX_SKIN_BYTES};
    use png::{BitDepth, ColorType, Encoder};

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
    fn builds_upload_and_public_texture_urls_from_the_api_base() {
        let (upload, image) = texture_urls("https://remote.example/api", "Player123").unwrap();
        assert_eq!(upload.as_str(), "https://remote.example/api/upload");
        assert_eq!(
            image.as_str(),
            "https://remote.example/uploads/users/Player123/skins/Player123.png"
        );
    }

    #[test]
    fn rejects_insecure_api_urls_and_invalid_nicknames() {
        assert!(texture_urls("http://remote.example/api", "Player123").is_err());
        assert!(texture_urls("https://remote.example/api", "../Player").is_err());
    }

    #[test]
    fn rejects_corrupt_png_data() {
        assert!(validate_skin_png(b"not a PNG").is_err());
    }
}
