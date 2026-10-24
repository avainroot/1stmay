import { mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
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
const releaseBaseUrl = readEnvValue("LAUNCHER_RELEASE_BASE_URL").replace(/\/+$/, "");
const privateKeyPath =
  process.env.TAURI_SIGNING_PRIVATE_KEY_PATH ??
  path.join(homedir(), ".tauri", "1stmay-launcher.key");
const privateKey = (
  process.env.TAURI_SIGNING_PRIVATE_KEY ?? readFileSync(privateKeyPath, "utf8")
).trim();
const publicKey = (
  process.env.TAURI_SIGNING_PUBLIC_KEY ?? readFileSync(`${privateKeyPath}.pub`, "utf8")
).trim();
if (!privateKey || !publicKey) {
  throw new Error("Не настроены ключи Tauri updater.");
}
const configDirectory = mkdtempSync(path.join(tmpdir(), "1stmay-release-"));
const configPath = path.join(configDirectory, "tauri.release.conf.json");
writeFileSync(
  configPath,
  JSON.stringify({
    plugins: {
      updater: {
        pubkey: publicKey,
        endpoints: [`${releaseBaseUrl}/manifest.json`],
      },
    },
  }),
);

let build;
try {
  build = spawnSync("pnpm", ["exec", "tauri", "build", "--config", configPath], {
    cwd: projectRoot,
    env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY: privateKey },
    shell: process.platform === "win32",
    stdio: "inherit",
  });
} finally {
  rmSync(configDirectory, { recursive: true, force: true });
}
if (build.status !== 0) {
  process.exit(build.status ?? 1);
}

const renameInstaller = spawnSync(
  process.execPath,
  [path.join(projectRoot, "scripts", "rename-installer.mjs")],
  { cwd: projectRoot, env: process.env, stdio: "inherit" },
);
if (renameInstaller.status !== 0) {
  process.exit(renameInstaller.status ?? 1);
}

const installerName = `${packageInfo.name}-${packageInfo.version}.exe`;
const installerSignatureSourceName = `${tauriConfig.productName}_${packageInfo.version}_x64-setup.exe.sig`;
const installerPath = path.join(bundleDirectory, installerName);
const installerSignaturePath = `${installerPath}.sig`;
rmSync(installerSignaturePath, { force: true });
renameSync(
  path.join(bundleDirectory, installerSignatureSourceName),
  installerSignaturePath,
);

for (const staleArchiveName of [
  `${packageInfo.name}-${packageInfo.version}.nsis.zip`,
  `${tauriConfig.productName}_${packageInfo.version}_x64-setup.nsis.zip`,
]) {
  const staleArchivePath = path.join(bundleDirectory, staleArchiveName);
  rmSync(staleArchivePath, { force: true });
  rmSync(`${staleArchivePath}.sig`, { force: true });
}

const signature = readFileSync(installerSignaturePath, "utf8").trim();
if (!signature) {
  throw new Error("Tauri создал пустую подпись updater-архива.");
}

const manifest = {
  version: packageInfo.version,
  notes: "",
  pub_date: new Date().toISOString(),
  url: `${releaseBaseUrl}/${installerName}`,
  signature,
};
const manifestPath = path.join(bundleDirectory, "manifest.json");
const temporaryManifestPath = `${manifestPath}.tmp`;
writeFileSync(temporaryManifestPath, `${JSON.stringify(manifest, null, 2)}\n`, {
  encoding: "utf8",
  flag: "w",
});
renameSync(temporaryManifestPath, manifestPath);

console.log(`Updater package: ${installerPath}`);
console.log(`Update manifest: ${manifestPath}`);

function readEnvValue(name) {
  const environmentValue = process.env[name];
  if (environmentValue) {
    validateUrl(name, environmentValue);
    return environmentValue;
  }

  const envFile = path.join(projectRoot, ".env");
  const line = readFileSync(envFile, "utf8")
    .split(/\r?\n/)
    .find((entry) => entry.trim().startsWith(`${name}=`));
  const value = line
    ?.slice(line.indexOf("=") + 1)
    .trim()
    .replace(/^(['"])(.*)\1$/, "$2");

  if (!value) {
    throw new Error(`В корневом .env не задан ${name}.`);
  }

  validateUrl(name, value);
  return value;
}

function validateUrl(name, value) {
  const url = new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error(`${name} должен быть HTTP(S) URL.`);
  }
}
