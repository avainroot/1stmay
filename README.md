;'-# 1stMay Launcher

The launcher targets Minecraft Java Edition `1.21.1` with NeoForge
`21.1.264` and Java `21`. NeoForge's official Maven installer artifact was
checked against its published SHA-256; its embedded `version.json` identifies
the profile as `neoforge-21.1.264`, inherits Minecraft `1.21.1`, and uses
`cpw.mods.bootstraplauncher.BootstrapLauncher`. The launcher reads that profile
and combines its launch arguments and libraries with the Mojang version
metadata; it does not use a third-party launcher library.

The project version is `1.2.1`. It targets 64-bit Windows 10 and Windows 11.
Windows 7/8 are not supported because the selected Java 21 runtime requires a
newer Windows release. Microsoft Edge WebView2 Runtime is also required; the
NSIS installer installs it when needed. Windows 10 and 11 compatibility is
documented by [Tauri](https://v2.tauri.app/reference/webview-versions/).
Minecraft's Java maximum heap is selected automatically up to 8 GiB based on
current free and total system memory. It uses the available memory even when
that is less than 4 GiB and leaves a proportional reserve for Windows and
other processes. The launcher shows total RAM, current free RAM, and the
selected Java heap limit.

Minecraft launches directly into multiplayer quick play at the configured
remote server, without opening the server menu. The game uses an offline
nickname profile. Russian is selected when the game has no language preference
or still has Minecraft's default `en_us`; an explicitly chosen non-default
language is preserved.

The offline UUID implementation is tested against the Java
`UUID.nameUUIDFromBytes` fixture for `Notch`. The install, launch and mod-sync
code has unit coverage.

Set `QUICK_PLAY_SERVER_ADDRESS` and `MOD_MANIFEST_URL` in a local root `.env`
file before building. Start from `.env.example`, replacing its placeholder
values with the deployment's remote server address and manifest URL. `.env`
is ignored by version control and the values are embedded into the built
client. Use HTTPS for the manifest: plain HTTP does not authenticate the
manifest or its hashes. VDS publication is managed separately from this
client repository.

## Checks

```sh
pnpm install --frozen-lockfile
pnpm run format:check
pnpm run build
pnpm run build:installer
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

The Windows x64 NSIS installer is emitted as
`firstmay-launcher-<version>.exe` in the Tauri release bundle directory.
The rename step is needed because Tauri's NSIS bundler names the installer
from the product name, version, architecture, and setup suffix; Cargo does
not configure the NSIS bundle filename.

The launcher checks for updates at startup and every six hours in the
background. It downloads a newer signed installer into the `1stMay-updates`
folder beside the installed launcher. The UI then shows the available version
and an **Обновить** button; clicking it asks Tauri to verify and install the
cached package. Windows restarts the launcher after installation. On the next
startup, the launcher removes its cached installer and metadata; it only deletes
its own named files and leaves other files untouched. `pnpm run build:release`
produces the NSIS installer, its Tauri updater signature, and a compatible
`manifest.json`. With `createUpdaterArtifacts: true`, Tauri signs the installer
itself; the manifest URL and signature therefore correspond to the `.exe`, not
an `.nsis.zip` archive. The updater endpoint is
`<LAUNCHER_RELEASE_BASE_URL>/manifest.json`; the current base is configured in
the local `.env` and GitHub Actions secret. The signing key is read from
`%USERPROFILE%\.tauri\1stmay-launcher.key` unless
`TAURI_SIGNING_PRIVATE_KEY_PATH` is set. Keep the private key outside the
repository and back it up securely. Versions before `1.1.0` predate the
in-app updater and need one manual installation of an updater-enabled release.
Subsequent releases can update through the launcher.

The **Проверить обновления** button manually starts the same check and
background download used at startup. The launcher can then install the downloaded
release using **Обновить**.

## GitHub Actions release deployment

`.github/workflows/build-deploy.yml` runs formatting, frontend build, and Rust
tests and builds the signed Windows NSIS release in one Windows job. A dependent
Ubuntu job syncs the installer and signature to the VDS with `rsync`, then
publishes the manifest atomically and removes older launcher versions. Configure
the required repository Actions secrets before pushing to `main` or starting
the workflow manually. Their descriptions and setup examples are kept in the
ignored local `AGENTS.md`, not in this README.

The workflow caches pnpm dependencies, Cargo dependencies/build outputs, and
the NSIS installation between runs. It writes build configuration only to the
temporary runner workspace and does not upload `.env`, source code, or signing
keys.
