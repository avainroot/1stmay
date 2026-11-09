use crate::{commands, config, settings};
use base64::{engine::general_purpose::STANDARD, Engine};
use png::Decoder;
use reqwest::{multipart, Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::{io::Cursor, path::Path, time::Duration};
use tauri::AppHandle;
use url::Url;

const MAX_SKIN_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextureType {
    Skins,
    Capes,
    Elytras,
}

impl TextureType {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "skins" => Ok(Self::Skins),
            "capes" => Ok(Self::Capes),
            "elytras" => Ok(Self::Elytras),
            _ => Err("Выберите тип текстуры skins, capes или elytras.".into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Skins => "skins",
            Self::Capes => "capes",
            Self::Elytras => "elytras",
        }
    }
}

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

#[derive(Deserialize)]
struct DeleteTextureResponse {
    success: bool,
    username: String,
    #[serde(rename = "type")]
    upload_type: String,
}

#[tauri::command]
pub async fn get_current_texture(
    app: AppHandle,
    texture_type: String,
) -> Result<Option<SkinPreview>, String> {
    let texture_type = TextureType::parse(&texture_type)?;
    let Some(nickname) = settings::load(&app)?.nickname else {
        return Ok(None);
    };
    commands::validate_nickname(&nickname)?;
    let (_, image_url) = texture_urls(config::TEXTURE_API_BASE_URL, &nickname, texture_type)?;
    let response = http_client()?
        .get(image_url)
        .send()
        .await
        .map_err(|error| format!("Не удалось получить текстуру из API: {error}"))?;

    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }

    let response = response
        .error_for_status()
        .map_err(|error| format!("Не удалось получить текстуру из API: {error}"))?;
    let bytes = read_bounded_response(response).await?;
    preview_from_bytes(&bytes, texture_type).map(Some)
}

#[tauri::command]
pub fn load_texture_preview(path: String, texture_type: String) -> Result<SkinPreview, String> {
    let texture_type = TextureType::parse(&texture_type)?;
    let bytes = read_valid_texture(Path::new(&path), texture_type)?;
    preview_from_bytes(&bytes, texture_type)
}

#[tauri::command]
pub async fn save_texture(
    app: AppHandle,
    path: String,
    texture_type: String,
) -> Result<(), String> {
    let texture_type = TextureType::parse(&texture_type)?;
    let nickname = current_nickname(&app)?;
    let bytes = read_valid_texture(Path::new(&path), texture_type)?;
    let (upload_url, _) = texture_urls(config::TEXTURE_API_BASE_URL, &nickname, texture_type)?;
    let file = multipart::Part::bytes(bytes)
        .file_name(format!("{nickname}.png"))
        .mime_str("image/png")
        .map_err(|error| format!("Не удалось подготовить PNG для загрузки: {error}"))?;
    let form = multipart::Form::new()
        .text("username", nickname.clone())
        .text("type", texture_type.as_str())
        .part("file", file);

    let response = http_client()?
        .post(upload_url)
        .multipart(form)
        .send()
        .await
        .map_err(|error| format!("Не удалось загрузить текстуру в API: {error}"))?;
    let response = response
        .error_for_status()
        .map_err(|error| format!("API не сохранил текстуру: {error}"))?;
    let result: UploadResponse = response
        .json()
        .await
        .map_err(|error| format!("API вернул некорректный ответ о загрузке скина: {error}"))?;

    if !result.success
        || result.username != nickname
        || result.upload_type != texture_type.as_str()
        || result.filename != format!("{nickname}.png")
    {
        return Err("API вернул неожиданный результат загрузки текстуры.".into());
    }

    Ok(())
}

#[tauri::command]
pub async fn delete_texture(app: AppHandle, texture_type: String) -> Result<(), String> {
    let texture_type = TextureType::parse(&texture_type)?;
    let nickname = current_nickname(&app)?;
    let (mut delete_url, _) = texture_urls(config::TEXTURE_API_BASE_URL, &nickname, texture_type)?;
    delete_url
        .query_pairs_mut()
        .append_pair("username", &nickname)
        .append_pair("type", texture_type.as_str());

    let response = http_client()?
        .delete(delete_url)
        .send()
        .await
        .map_err(|error| format!("Не удалось удалить текстуру через API: {error}"))?;
    let response = response
        .error_for_status()
        .map_err(|error| format!("API не удалил текстуру: {error}"))?;
    let result: DeleteTextureResponse = response
        .json()
        .await
        .map_err(|error| format!("API вернул некорректный ответ об удалении текстуры: {error}"))?;

    if !result.success || result.username != nickname || result.upload_type != texture_type.as_str()
    {
        return Err("API вернул неожиданный результат удаления текстуры.".into());
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

fn texture_urls(
    api_base: &str,
    nickname: &str,
    texture_type: TextureType,
) -> Result<(Url, Url), String> {
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
        "/uploads/users/{nickname}/{}/{nickname}.png",
        texture_type.as_str()
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

fn read_valid_texture(path: &Path, texture_type: TextureType) -> Result<Vec<u8>, String> {
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
    validate_texture_png(&bytes, texture_type)?;
    Ok(bytes)
}

fn validate_texture_png(bytes: &[u8], texture_type: TextureType) -> Result<(), String> {
    if bytes.len() > MAX_SKIN_BYTES {
        return Err("PNG скина превышает допустимый размер 2 МиБ.".into());
    }
    let mut reader = Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(|error| format!("Файл не является корректным PNG: {error}"))?;
    let (width, height) = (reader.info().width, reader.info().height);
    match texture_type {
        TextureType::Skins if !matches!((width, height), (64, 64) | (64, 32)) => {
            return Err(format!(
                "Размер скина {width}×{height} не поддерживается. Используйте PNG 64×64 или 64×32."
            ));
        }
        TextureType::Capes | TextureType::Elytras
            if !(u64::from(width) == 2 * u64::from(height)
                || u64::from(width) * 17 == u64::from(height) * 22
                || u64::from(width) * 11 == u64::from(height) * 23) =>
        {
            return Err(format!(
                "Размер плаща/элитры {width}×{height} не поддерживается 3D-просмотрщиком. Допустимые пропорции: 64×32, 22×17 или 46×22."
            ));
        }
        _ => {}
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

fn preview_from_bytes(bytes: &[u8], texture_type: TextureType) -> Result<SkinPreview, String> {
    validate_texture_png(bytes, texture_type)?;
    Ok(SkinPreview {
        data_url: format!("data:image/png;base64,{}", STANDARD.encode(bytes)),
    })
}

#[cfg(test)]
mod tests {
    use super::{texture_urls, validate_texture_png, TextureType, MAX_SKIN_BYTES};
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
        assert!(
            validate_texture_png(&png_bytes(64, 64, [10, 20, 30, 255]), TextureType::Skins).is_ok()
        );
        assert!(
            validate_texture_png(&png_bytes(64, 32, [10, 20, 30, 255]), TextureType::Skins).is_ok()
        );
    }

    #[test]
    fn accepts_cape_dimensions_supported_by_skinview3d() {
        for (width, height) in [(64, 32), (22, 17), (46, 22)] {
            let png = png_bytes(width, height, [10, 20, 30, 255]);
            assert!(validate_texture_png(&png, TextureType::Capes).is_ok());
            assert!(validate_texture_png(&png, TextureType::Elytras).is_ok());
        }
    }

    #[test]
    fn rejects_invalid_dimensions_and_oversized_pngs() {
        assert!(
            validate_texture_png(&png_bytes(128, 128, [0, 0, 0, 255]), TextureType::Skins).is_err()
        );
        assert!(
            validate_texture_png(&png_bytes(32, 32, [0, 0, 0, 255]), TextureType::Capes).is_err()
        );
        assert!(validate_texture_png(&vec![0; MAX_SKIN_BYTES + 1], TextureType::Skins).is_err());
    }

    #[test]
    fn builds_upload_and_public_texture_urls_from_the_api_base() {
        for texture_type in [TextureType::Skins, TextureType::Capes, TextureType::Elytras] {
            let (upload, image) =
                texture_urls("https://remote.example/api", "Player123", texture_type).unwrap();
            assert_eq!(upload.as_str(), "https://remote.example/api/upload");
            assert_eq!(
                image.as_str(),
                format!(
                    "https://remote.example/uploads/users/Player123/{}/Player123.png",
                    texture_type.as_str()
                )
            );
            let mut delete = upload;
            delete
                .query_pairs_mut()
                .append_pair("username", "Player123")
                .append_pair("type", texture_type.as_str());
            assert_eq!(
                delete.as_str(),
                format!(
                    "https://remote.example/api/upload?username=Player123&type={}",
                    texture_type.as_str()
                )
            );
        }
    }

    #[test]
    fn rejects_insecure_api_urls_and_invalid_nicknames() {
        assert!(
            texture_urls("http://remote.example/api", "Player123", TextureType::Skins).is_err()
        );
        assert!(texture_urls(
            "https://remote.example/api",
            "../Player",
            TextureType::Skins
        )
        .is_err());
    }

    #[test]
    fn rejects_corrupt_png_data() {
        assert!(validate_texture_png(b"not a PNG", TextureType::Skins).is_err());
    }
}
