import { copyFileSync, existsSync, readdirSync, unlinkSync } from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import packageInfo from "../package.json" with { type: "json" };
import tauriConfig from "../src-tauri/tauri.conf.json" with { type: "json" };

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const cargoMetadata = spawnSync(
  "cargo",
  [
    "metadata",
    "--manifest-path",
    path.join(projectRoot, "src-tauri", "Cargo.toml"),
    "--no-deps",
    "--format-version",
    "1",
  ],
  { cwd: projectRoot, encoding: "utf8" },
);
if (cargoMetadata.status !== 0) {
  throw new Error("Не удалось определить каталог сборки Cargo.");
}
const cargoTargetDirectory = JSON.parse(cargoMetadata.stdout).target_directory;
const targetRoot = path.resolve(
  projectRoot,
  process.env.CARGO_TARGET_DIR ?? cargoTargetDirectory,
);
const bundleDirectory = path.join(targetRoot, "release", "bundle", "nsis");
const expectedInstaller = `${tauriConfig.productName}_${packageInfo.version}_x64-setup.exe`;
const source = path.join(bundleDirectory, expectedInstaller);
const destination = path.join(
  bundleDirectory,
  `${packageInfo.name}-${packageInfo.version}.exe`,
);

if (!existsSync(source)) {
  const available = existsSync(bundleDirectory)
    ? readdirSync(bundleDirectory).join(", ")
    : "каталог установщика отсутствует";
  throw new Error(`Сборка Tauri не создала ${expectedInstaller}. Найдено: ${available}`);
}

copyFileSync(source, destination);
unlinkSync(source);
console.log(`Installer: ${destination}`);
