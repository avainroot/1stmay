use crate::{config::JAVA_MAJOR_VERSION, settings};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Cursor},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use zip::ZipArchive;

#[derive(Clone, Deserialize)]
struct AdoptiumAsset {
    binary: AdoptiumBinary,
}

#[derive(Clone, Deserialize)]
struct AdoptiumBinary {
    package: AdoptiumPackage,
}

#[derive(Clone, Deserialize)]
struct AdoptiumPackage {
    link: String,
    checksum: String,
    size: u64,
}

pub async fn ensure_java(app: &AppHandle) -> Result<PathBuf, String> {
    let root = settings::settings_directory(app)?;
    let runtime_root = root.join("runtime");
    fs::create_dir_all(&runtime_root)
        .map_err(|error| format!("Не удалось создать папку Java Runtime: {error}"))?;
    if let Some(existing) = find_installed_java(&runtime_root) {
        return Ok(existing);
    }

    emit_progress(app, "Подготовка Java Runtime 21…")?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|error| format!("Не удалось подготовить загрузчик Java: {error}"))?;
    let api_url = format!(
        "https://api.adoptium.net/v3/assets/latest/{JAVA_MAJOR_VERSION}/hotspot?architecture=x64&heap_size=normal&image_type=jre&jvm_impl=hotspot&os=windows&vendor=eclipse"
    );
    let assets: Vec<AdoptiumAsset> = client
        .get(api_url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Не удалось получить метаданные Java 21: {error}"))?
        .json()
        .await
        .map_err(|error| format!("Ответ Adoptium для Java 21 имеет неверный формат: {error}"))?;
    let package = assets
        .first()
        .ok_or_else(|| "Adoptium не предоставил архив Java Runtime 21 для Windows x64".to_string())?
        .binary
        .package
        .clone();

    if !package.link.starts_with("https://") || package.checksum.len() != 64 {
        return Err("Метаданные Java Runtime содержат недопустимую ссылку или SHA-256".into());
    }
    let final_dir = runtime_root.join(format!("temurin-21-{}", &package.checksum[..12]));
    let java_path = final_dir.join("bin").join("javaw.exe");
    if java_path.is_file() {
        return Ok(java_path);
    }

    let archive = client
        .get(&package.link)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Не удалось скачать Java Runtime 21: {error}"))?
        .bytes()
        .await
        .map_err(|error| format!("Загрузка Java Runtime прервалась: {error}"))?;
    if archive.len() as u64 != package.size {
        return Err(format!(
            "Размер архива Java не совпал: ожидалось {} байт, получено {}",
            package.size,
            archive.len()
        ));
    }
    let actual_hash = hex::encode(Sha256::digest(&archive));
    if actual_hash != package.checksum.to_ascii_lowercase() {
        return Err("SHA-256 архива Java Runtime не совпал с метаданными Adoptium".into());
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("Системное время недоступно: {error}"))?
        .as_nanos();
    let staging = runtime_root.join(format!("extract-{}-{stamp}", std::process::id()));
    fs::create_dir(&staging)
        .map_err(|error| format!("Не удалось подготовить установку Java: {error}"))?;
    if let Err(error) = extract_runtime(&archive, &staging) {
        return Err(format!("Не удалось распаковать Java Runtime: {error}"));
    }

    if final_dir.exists() {
        return Err(format!(
            "Папка Java {} уже существует, но не содержит javaw.exe; проверьте её вручную.",
            final_dir.display()
        ));
    }
    fs::rename(&staging, &final_dir)
        .map_err(|error| format!("Не удалось установить Java Runtime: {error}"))?;
    if !java_path.is_file() {
        return Err("Архив Java распакован, но javaw.exe не найден".into());
    }
    Ok(java_path)
}

fn find_installed_java(runtime_root: &Path) -> Option<PathBuf> {
    let mut found = fs::read_dir(runtime_root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("bin").join("javaw.exe"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    found.sort();
    found.pop()
}

fn emit_progress(app: &AppHandle, message: &str) -> Result<(), String> {
    app.emit(
        "launcher-progress",
        serde_json::json!({ "message": message }),
    )
    .map_err(|error| format!("Не удалось обновить статус лаунчера: {error}"))
}

fn extract_runtime(bytes: &[u8], destination: &Path) -> io::Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let Some(path) = entry.enclosed_name() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "архив содержит путь за пределами каталога",
            ));
        };
        let relative = path.components().skip(1).collect::<PathBuf>();
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::File::create(&target)?;
        io::copy(&mut entry, &mut output)?;
        output.sync_all()?;
    }
    Ok(())
}
