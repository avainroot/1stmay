import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const releaseBaseUrl = readSetting("LAUNCHER_RELEASE_BASE_URL");
const url = new URL(releaseBaseUrl);
if (url.protocol !== "https:") {
  throw new Error("LAUNCHER_RELEASE_BASE_URL must use HTTPS.");
}

const publicKeyPath = path.join(homedir(), ".tauri", "1stmay-launcher.key.pub");
const publicKey =
  process.env.TAURI_SIGNING_PUBLIC_KEY || readFileSync(publicKeyPath, "utf8").trim();
if (!publicKey) {
  throw new Error(`Tauri updater public key is empty: ${publicKeyPath}`);
}

const configDirectory = mkdtempSync(path.join(tmpdir(), "1stmay-dev-"));
const configPath = path.join(configDirectory, "tauri.dev.conf.json");
writeFileSync(
  configPath,
  JSON.stringify({
    plugins: {
      updater: {
        pubkey: publicKey,
        endpoints: [`${releaseBaseUrl.replace(/\/+$/, "")}/manifest.json`],
      },
    },
  }),
);

try {
  const result = spawnSync("pnpm", ["exec", "tauri", "dev", "--config", configPath], {
    cwd: projectRoot,
    env: process.env,
    shell: process.platform === "win32",
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} finally {
  rmSync(configDirectory, { recursive: true, force: true });
}

function readSetting(name) {
  const value = process.env[name] || readEnvFileValue(name);
  if (!value) {
    throw new Error(`${name} must be set in the environment or local .env file.`);
  }
  return value;
}

function readEnvFileValue(name) {
  const envPath = path.join(projectRoot, ".env");
  let contents;
  try {
    contents = readFileSync(envPath, "utf8");
  } catch (error) {
    if (error.code === "ENOENT") return "";
    throw error;
  }
  const line = contents
    .split(/\r?\n/)
    .find((entry) => entry.trim().startsWith(`${name}=`));
  return (
    line
      ?.slice(line.indexOf("=") + 1)
      .trim()
      .replace(/^(['"])(.*)\1$/, "$2") ?? ""
  );
}
