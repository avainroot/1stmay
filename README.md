;'-# 1stMay Launcher

The launcher targets Minecraft Java Edition `1.21.1` with NeoForge
`21.1.265` and Java `21`. NeoForge's official Maven installer artifact was
checked against its published SHA-256; its embedded `version.json` identifies
the profile as `neoforge-21.1.265`, inherits Minecraft `1.21.1`, and uses
`cpw.mods.bootstraplauncher.BootstrapLauncher`. The launcher reads that profile
and combines its launch arguments and libraries with the Mojang version
metadata; it does not use a third-party launcher library.

The project version is `1.5.0`. It targets 64-bit Windows 10 and Windows 11.
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

If the NeoForge installer fails, the action console includes the installer
output from both stdout and stderr to help diagnose the cause. NeoForge
`21.1.265` also requires its `universal` JAR as a game-layer module; the
launcher verifies its pinned size and SHA-1, downloads it before running the
installer, and leaves discovery to NeoForge's game-layer locator. If the official download times
out, the launcher derives a sibling `-universal.jar` URL from `NEOFORGE_URL`,
so the configured HTTPS mirror must publish both the installer and that
universal artifact. An incomplete profile without the verified universal JAR
is treated as not installed and is repaired before launch.

Set `QUICK_PLAY_SERVER_ADDRESS`, `MOD_MANIFEST_URL`, `NEOFORGE_URL`, and
`LAUNCHER_RELEASE_BASE_URL` in a local root `.env` file. Start from
`.env.example`, replacing its placeholder values with the deployment's remote
server address, shared file manifest URL (for example
`https://mc.rootflow.ru/manifest.json`), fallback NeoForge installer URL, and
release base URL. If the official NeoForge installer download times out after
8 seconds, the launcher retries it through `NEOFORGE_URL`; both sources are
verified against the pinned installer SHA-256. Set `NEOFORGE_URL` as a
GitHub Actions secret for release builds. The manifest uses
schema version `1` with a top-level `files` array and entries containing
`name`, `size`, `sha256`, and a path under `minecraft/`. `.env`
is ignored by version control and the values are embedded into the built
client. Use HTTPS for the manifest: plain HTTP does not authenticate the
manifest or its hashes. VDS publication is managed separately from this
client repository.

The action console retains up to 100 recent events, wraps long messages, and
automatically scrolls to the latest entry. URLs are redacted from console
messages so configured manifest, updater, and server addresses are not shown.

Player skins, capes, and elytras use the NestJS texture API configured by
`TEXTURE_API_BASE_URL` in the local `.env` (for example,
`https://remote.example/api`). Choose a texture type to load a PNG preview,
upload it to `POST <base>/upload`, or delete only that type through
`DELETE <base>/upload?username=<nickname>&type=<type>`. The saved Minecraft
nickname is used as the API username. The 3D preview renders skins and the
selected cape or elytra with `skinview3d`; skins must be 64×64 or 64×32, while
capes and elytras must use the viewer-supported 2:1, 22:17, or 46:22 image
proportions. When no skin exists, the viewer shows a neutral model. Textures
are not copied into `%APPDATA%` or the Minecraft game directory, and no texture
sync runs before launch. CustomSkinLoader must be configured separately to
request the public URLs
`<origin>/uploads/users/<nickname>/(skins|capes|elytras)/<nickname>.png`.
The API currently has no authentication: any client can overwrite or delete
textures for any nickname, so these controls are not protected from other
clients until authentication is added.

On the first successful Play, the launcher synchronizes every file listed
under `minecraft/` in the shared manifest, including `mods`, `config`,
`shaderpacks`, `fancymenu_data`, and other directories such as `data`. It stores
`config_synced: true` in
`%APPDATA%\\1stMay\\settings.json` only after configuration synchronization
completes. Later launches sync only `mods`. The **Синхронизировать конфиги**
button forces another hash-checked sync of all other listed game files. That
operation replaces changed files listed by the server manifest but does not
delete local files absent from the manifest.

The shared manifest endpoint lists files beneath the Minecraft source
directory in its `files` array. Any safe relative path under `minecraft/` is
synced to the corresponding location under the game directory; `name` must be
the file's basename. Paths are still validated to prevent traversal outside
the game directory. These entries are consumed from the same `manifest.json`;
there is no separate configuration manifest.

Run `pnpm run dev:tauri` to start the desktop app with updater checks enabled.
The dev runner builds a temporary Tauri config from `LAUNCHER_RELEASE_BASE_URL`
and the public key in `%USERPROFILE%\\.tauri\\1stmay-launcher.key.pub` (or
`TAURI_SIGNING_PUBLIC_KEY` if set). The temporary config is removed when the
dev process exits; neither value is written to tracked files.

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
