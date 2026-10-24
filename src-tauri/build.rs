use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let env_file = manifest_dir.join("../.env");
    println!("cargo:rerun-if-changed={}", env_file.display());
    for name in ["QUICK_PLAY_SERVER_ADDRESS", "MOD_MANIFEST_URL"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    dotenvy::from_path(&env_file).unwrap_or_else(|error| {
        panic!(
            "Не удалось загрузить {}: {error}. Создайте .env на основе .env.example.",
            env_file.display()
        )
    });
    for name in ["QUICK_PLAY_SERVER_ADDRESS", "MOD_MANIFEST_URL"] {
        let value = env::var(name).unwrap_or_else(|_| panic!("В .env не задан {name}"));
        println!("cargo:rustc-env={name}={value}");
    }
    tauri_build::build()
}
