use crate::{
    config, memory,
    metadata::{self, Argument, ArgumentValue, VersionMetadata},
    minecraft, settings,
};
use md5::{Digest, Md5};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    time,
};
use uuid::Uuid;

static GAME_RUNNING: AtomicBool = AtomicBool::new(false);

pub async fn launch(
    app: &AppHandle,
    game_dir: &Path,
    nickname: &str,
    java: &Path,
) -> Result<bool, String> {
    GAME_RUNNING.store(false, Ordering::Release);
    let parent_path = game_dir
        .join("versions")
        .join(config::MINECRAFT_VERSION)
        .join(format!("{}.json", config::MINECRAFT_VERSION));
    let parent: VersionMetadata = serde_json::from_slice(
        &fs::read(&parent_path)
            .map_err(|error| format!("Не удалось прочитать метаданные Minecraft: {error}"))?,
    )
    .map_err(|error| format!("Метаданные Minecraft повреждены: {error}"))?;
    let profile = minecraft::load_profile(game_dir)?;
    if profile.main_class.is_empty() {
        return Err("В профиле NeoForge отсутствует главный класс".into());
    }

    let classpath = build_classpath(game_dir, &parent, &profile)?;
    let natives_dir = settings::settings_directory(app)?
        .join("cache")
        .join(format!("natives-{}", config::NEOFORGE_PROFILE_ID));
    if !natives_dir.is_dir() {
        return Err("Каталог native-библиотек Minecraft не найден".into());
    }
    let asset_index = parent
        .asset_index
        .as_ref()
        .ok_or_else(|| "В метаданных Minecraft отсутствует индекс ресурсов".to_string())?;
    let heap = memory::available_heap_mib()?;

    let game_values = HashMap::from([
        ("${auth_player_name}", nickname.to_string()),
        ("${version_name}", profile.id.clone()),
        ("${game_directory}", game_dir.to_string_lossy().into_owned()),
        (
            "${assets_root}",
            game_dir.join("assets").to_string_lossy().into_owned(),
        ),
        ("${assets_index_name}", asset_index.id.clone()),
        ("${auth_uuid}", offline_uuid(nickname).simple().to_string()),
        ("${auth_access_token}", "0".into()),
        ("${clientid}", String::new()),
        ("${auth_xuid}", String::new()),
        ("${user_type}", "legacy".into()),
        ("${version_type}", "release".into()),
        (
            "${natives_directory}",
            natives_dir.to_string_lossy().into_owned(),
        ),
        ("${launcher_name}", "1stMay Server".into()),
        ("${launcher_version}", env!("CARGO_PKG_VERSION").into()),
        ("${classpath}", classpath),
        ("${classpath_separator}", ";".into()),
        (
            "${library_directory}",
            game_dir.join("libraries").to_string_lossy().into_owned(),
        ),
    ]);
    let mut jvm_values = game_values.clone();
    jvm_values.insert("${version_name}", config::MINECRAFT_VERSION.to_string());
    let mut jvm_args = collect_arguments(&parent, true)?;
    jvm_args.extend(collect_arguments(&profile, true)?);
    jvm_args.retain(|argument| !argument.starts_with("-Xms") && !argument.starts_with("-Xmx"));
    jvm_args.push(format!("-Xms{heap}M"));
    jvm_args.push(format!("-Xmx{heap}M"));
    let mut game_args = collect_arguments(&parent, false)?;
    game_args.extend(collect_arguments(&profile, false)?);

    let jvm_args = resolve_args(jvm_args, &jvm_values)?;
    let mut game_args = resolve_args(game_args, &game_values)?;
    game_args.extend(quick_play_args(config::QUICK_PLAY_SERVER_ADDRESS));
    ensure_default_language(game_dir)?;
    let mut command = Command::new(java);
    command
        .args(jvm_args)
        .arg(&profile.main_class)
        .args(game_args)
        .current_dir(game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    emit_progress(
        app,
        "Запуск Minecraft и переход на сервер через quick play…",
    )?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("Не удалось запустить Java Runtime: {error}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Не удалось получить вывод Java для диагностики".to_string())?;
    let stderr_task = tokio::spawn(async move {
        let mut stderr = stderr;
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).await.map(|_| output)
    });

    for _ in 0..20 {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Не удалось проверить запуск Minecraft: {error}"))?
        {
            let output = stderr_task
                .await
                .map_err(|error| format!("Не удалось прочитать вывод Java: {error}"))?
                .map_err(|error| format!("Не удалось прочитать вывод Java: {error}"))?;
            return if status.success() {
                emit_game_state(app, false, "Игра закрылась во время запуска.")?;
                Ok(false)
            } else {
                Err(format!(
                    "Minecraft завершился при запуске ({}): {}",
                    status,
                    truncate(&String::from_utf8_lossy(&output), 3000)
                ))
            };
        }
        time::sleep(Duration::from_millis(250)).await;
    }

    GAME_RUNNING.store(true, Ordering::Release);
    emit_game_state(app, true, "Игра запущена.")?;
    monitor_child(app.clone(), child, stderr_task);
    Ok(true)
}

fn quick_play_args(server_address: &str) -> Vec<String> {
    vec!["--quickPlayMultiplayer".into(), server_address.to_string()]
}

pub fn is_game_running() -> bool {
    GAME_RUNNING.load(Ordering::Acquire)
}

fn ensure_default_language(game_dir: &Path) -> Result<(), String> {
    let options_path = game_dir.join("options.txt");
    let mut options = match fs::read_to_string(&options_path) {
        Ok(options) => options,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "Не удалось прочитать настройки Minecraft {}: {error}",
                options_path.display()
            ))
        }
    };

    let language = options
        .lines()
        .find_map(|line| line.strip_prefix("lang:").map(str::to_string));
    match language.as_deref() {
        Some("ru_ru") => return Ok(()),
        Some("en_us") => {
            let mut updated = String::with_capacity(options.len());
            for line in options.split_inclusive('\n') {
                let content = line.trim_end_matches(&['\r', '\n'][..]);
                if content == "lang:en_us" {
                    updated.push_str("lang:ru_ru");
                    updated.push_str(&line[content.len()..]);
                } else {
                    updated.push_str(line);
                }
            }
            options = updated;
        }
        Some(_) => return Ok(()),
        None => options.push_str("lang:ru_ru\n"),
    }
    if !options.is_empty() && !options.ends_with('\n') {
        options.push('\n');
    }
    if !options.ends_with('\n') {
        options.push('\n');
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("Системное время недоступно: {error}"))?
        .as_nanos();
    let temporary = options_path.with_extension(format!("tmp-{}-{stamp}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("Не удалось подготовить язык Minecraft: {error}"))?;
    file.write_all(options.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Не удалось записать язык Minecraft: {error}"))?;
    drop(file);
    replace_file(&temporary, &options_path)
        .map_err(|error| format!("Не удалось сохранить язык Minecraft: {error}"))
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

fn build_classpath(
    game_dir: &Path,
    parent: &VersionMetadata,
    profile: &VersionMetadata,
) -> Result<String, String> {
    let mut order = Vec::new();
    let mut libraries = HashMap::new();
    for library in parent.libraries.iter().chain(profile.libraries.iter()) {
        if !metadata::supported_library(library) {
            continue;
        }
        let key = library_key(&library.name);
        if !libraries.contains_key(&key) {
            order.push(key.clone());
        }
        libraries.insert(key, library);
    }

    let mut paths = Vec::<PathBuf>::new();
    for key in order {
        let Some(library) = libraries.get(&key) else {
            continue;
        };
        let Some(downloads) = &library.downloads else {
            continue;
        };
        let Some(artifact) = &downloads.artifact else {
            continue;
        };
        let relative = artifact
            .path
            .as_deref()
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| metadata::coordinate_path(&library.name))?;
        let path = game_dir.join("libraries").join(relative);
        if !path.is_file() {
            return Err(format!(
                "Не найдена библиотека {}: {}",
                library.name,
                path.display()
            ));
        }
        paths.push(path);
    }
    paths.push(
        game_dir
            .join("versions")
            .join(config::MINECRAFT_VERSION)
            .join(format!("{}.jar", config::MINECRAFT_VERSION)),
    );
    let mut seen = std::collections::HashSet::new();
    let joined = paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(";");
    if joined.is_empty() {
        return Err("Для запуска не сформирован classpath".into());
    }
    Ok(joined)
}

fn library_key(name: &str) -> String {
    let coordinate = name.split('@').next().unwrap_or(name);
    let parts = coordinate.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        [group, artifact, _version] => format!("{group}:{artifact}"),
        [group, artifact, _version, classifier] => {
            format!("{group}:{artifact}:{classifier}")
        }
        _ => name.to_string(),
    }
}

fn collect_arguments(metadata: &VersionMetadata, jvm: bool) -> Result<Vec<String>, String> {
    let arguments = metadata
        .arguments
        .as_ref()
        .ok_or_else(|| format!("В метаданных {} отсутствует блок arguments", metadata.id))?;
    let source = if jvm { &arguments.jvm } else { &arguments.game };
    let mut collected = Vec::new();
    for argument in source {
        match argument {
            Argument::Value(value) => collected.push(value.clone()),
            Argument::Values(values) => collected.extend(values.clone()),
            Argument::Conditional(conditional) => {
                if metadata::library_allowed(Some(&conditional.rules)) {
                    match &conditional.value {
                        ArgumentValue::Value(value) => collected.push(value.clone()),
                        ArgumentValue::Values(values) => collected.extend(values.clone()),
                    }
                }
            }
        }
    }
    Ok(collected)
}

fn resolve_args(args: Vec<String>, values: &HashMap<&str, String>) -> Result<Vec<String>, String> {
    args.into_iter()
        .map(|mut argument| {
            for (placeholder, value) in values {
                argument = argument.replace(placeholder, value);
            }
            if argument.contains("${") {
                return Err(format!("Не удалось разрешить аргумент запуска: {argument}"));
            }
            Ok(argument)
        })
        .collect()
}

pub fn offline_uuid(nickname: &str) -> Uuid {
    let mut digest = Md5::digest(format!("OfflinePlayer:{nickname}").as_bytes());
    digest[6] = (digest[6] & 0x0f) | 0x30;
    digest[8] = (digest[8] & 0x3f) | 0x80;
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest);
    Uuid::from_bytes(bytes)
}

fn monitor_child(
    app: AppHandle,
    mut child: Child,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) {
    tokio::spawn(async move {
        let message = match child.wait().await {
            Ok(status) if status.success() => "Игра завершена.".to_string(),
            Ok(status) => {
                let detail = match stderr_task.await {
                    Ok(Ok(output)) => truncate(&String::from_utf8_lossy(&output), 3000),
                    Ok(Err(error)) => format!("Не удалось прочитать вывод Java: {error}"),
                    Err(error) => format!("Не удалось дождаться вывода Java: {error}"),
                };
                format!("Игра завершилась с ошибкой ({status}): {detail}")
            }
            Err(error) => format!("Не удалось отследить процесс игры: {error}"),
        };
        GAME_RUNNING.store(false, Ordering::Release);
        let _ = emit_game_state(&app, false, &message);
    });
}

fn emit_game_state(app: &AppHandle, running: bool, message: &str) -> Result<(), String> {
    app.emit(
        "launcher-game-state",
        GameState {
            running,
            message: message.to_string(),
        },
    )
    .map_err(|error| format!("Не удалось обновить состояние игры: {error}"))
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().rev().take(max_chars).collect::<String>();
    output = output.chars().rev().collect();
    if value.chars().count() > max_chars {
        output.insert(0, '…');
    }
    output
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

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GameState {
    running: bool,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::{ensure_default_language, offline_uuid, quick_play_args, resolve_args};
    use crate::config;
    use std::{collections::HashMap, fs};

    #[test]
    fn offline_uuid_matches_java_name_uuid_from_bytes_fixture() {
        assert_eq!(
            offline_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn library_key_ignores_version_so_neoforge_can_replace_vanilla_jars() {
        assert_eq!(
            super::library_key("org.ow2.asm:asm:9.3"),
            super::library_key("org.ow2.asm:asm:9.10.1")
        );
        assert_ne!(
            super::library_key("org.lwjgl:lwjgl:3.3.3"),
            super::library_key("org.lwjgl:lwjgl:3.3.3:natives-windows")
        );
    }

    #[test]
    fn neoforge_ignore_list_uses_the_inherited_minecraft_jar_name() {
        let game_values = HashMap::from([("${version_name}", "neoforge-21.1.264".to_string())]);
        let mut jvm_values = game_values.clone();
        jvm_values.insert("${version_name}", config::MINECRAFT_VERSION.to_string());

        assert_eq!(
            resolve_args(
                vec!["-DignoreList=client-extra,${version_name}.jar".into()],
                &jvm_values,
            )
            .unwrap(),
            ["-DignoreList=client-extra,1.21.1.jar"]
        );
        assert_eq!(
            resolve_args(vec!["${version_name}".into()], &game_values).unwrap(),
            ["neoforge-21.1.264"]
        );
    }

    #[test]
    fn quick_play_uses_the_configured_multiplayer_address() {
        assert_eq!(
            quick_play_args(config::QUICK_PLAY_SERVER_ADDRESS),
            ["--quickPlayMultiplayer", env!("QUICK_PLAY_SERVER_ADDRESS")]
        );
    }

    #[test]
    fn russian_language_is_only_added_when_no_language_is_selected() {
        let directory = tempfile::tempdir().unwrap();
        let options = directory.path().join("options.txt");
        ensure_default_language(directory.path()).unwrap();
        assert_eq!(fs::read_to_string(&options).unwrap(), "lang:ru_ru\n");

        fs::write(&options, "sound:1.0\nlang:en_us\n").unwrap();
        ensure_default_language(directory.path()).unwrap();
        assert_eq!(
            fs::read_to_string(&options).unwrap(),
            "sound:1.0\nlang:ru_ru\n"
        );

        fs::write(&options, "sound:1.0\nlang:fr_fr\n").unwrap();
        ensure_default_language(directory.path()).unwrap();
        assert_eq!(
            fs::read_to_string(&options).unwrap(),
            "sound:1.0\nlang:fr_fr\n"
        );
    }
}
