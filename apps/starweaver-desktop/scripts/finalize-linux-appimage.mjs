import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const APPIMAGE_PLUGIN_URL =
  "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/1-alpha-20250213-1/linuxdeploy-plugin-appimage-x86_64.AppImage";
const APPIMAGE_PLUGIN_SHA256 = "992d502a248e14ab185448ddf6f6e7d25558cb84d4623c354c3af350c25fccb3";
const APPIMAGE_PLUGIN_SIZE = 15_889_136;

function fail(message) {
  console.error(`finalize-linux-appimage: ${message}`);
  process.exit(1);
}

function argument(name) {
  const index = process.argv.indexOf(name);
  const value = index >= 0 ? process.argv[index + 1] : undefined;
  if (!value || value.startsWith("--")) fail(`${name} is required`);
  return value;
}

function entries(directory, predicate) {
  return readdirSync(directory, { withFileTypes: true })
    .filter(predicate)
    .map((entry) => join(directory, entry.name));
}

function one(paths, label) {
  if (paths.length !== 1) fail(`expected one ${label}, found ${paths.length}`);
  return paths[0];
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function verifiedAppImagePlugin(repositoryRoot) {
  const plugin = join(
    repositoryRoot,
    "target",
    "desktop-tools",
    "linuxdeploy-plugin-appimage-x86_64.AppImage",
  );
  if (!existsSync(plugin)) {
    const response = await fetch(APPIMAGE_PLUGIN_URL, { redirect: "follow" });
    if (!response.ok) fail("could not download the pinned AppImage output plugin");
    const bytes = Buffer.from(await response.arrayBuffer());
    if (bytes.length !== APPIMAGE_PLUGIN_SIZE || sha256(bytes) !== APPIMAGE_PLUGIN_SHA256) {
      fail("downloaded AppImage output plugin failed its pinned digest or size check");
    }
    mkdirSync(dirname(plugin), { mode: 0o700, recursive: true });
    const temporary = `${plugin}.${process.pid.toString()}.tmp`;
    try {
      writeFileSync(temporary, bytes, { mode: 0o700 });
      renameSync(temporary, plugin);
    } finally {
      rmSync(temporary, { force: true });
    }
  }
  const bytes = readFileSync(plugin);
  if (
    !statSync(plugin).isFile() ||
    bytes.length !== APPIMAGE_PLUGIN_SIZE ||
    sha256(bytes) !== APPIMAGE_PLUGIN_SHA256
  ) {
    fail("cached AppImage output plugin failed its pinned digest or size check");
  }
  chmodSync(plugin, 0o700);
  return plugin;
}

function filesBelow(root) {
  const pending = [root];
  const files = [];
  while (pending.length > 0) {
    const directory = pending.pop();
    if (directory === undefined) break;
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) pending.push(path);
      else if (entry.isFile()) files.push(path);
    }
  }
  return files;
}

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(scriptDirectory, "../..");
const target = argument("--target");
const supportedTargets = new Set([
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "x86_64-unknown-linux-gnu",
]);
if (!supportedTargets.has(target)) fail(`unsupported Desktop target: ${target}`);
if (target !== "x86_64-unknown-linux-gnu") process.exit(0);

const bundleRoot = resolve(argument("--bundle-root"));
const binary = resolve(argument("--binary"));
if (basename(binary) !== "starweaver-rpc" || !existsSync(binary) || !statSync(binary).isFile()) {
  fail("--binary must be the exact Linux starweaver-rpc target build");
}

const appImageRoot = join(bundleRoot, "appimage");
const appDir = one(
  entries(appImageRoot, (entry) => entry.isDirectory() && entry.name.endsWith(".AppDir")),
  "AppDir",
);
const appImage = one(
  entries(appImageRoot, (entry) => entry.isFile() && entry.name.endsWith(".AppImage")),
  "AppImage",
);
const sidecar = one(
  filesBelow(appDir).filter((path) => basename(path) === "starweaver-rpc"),
  "AppDir RPC sidecar",
);

const replacement = `${sidecar}.${process.pid.toString()}.tmp`;
const hadSignature = existsSync(`${appImage}.sig`);
try {
  copyFileSync(binary, replacement);
  chmodSync(replacement, statSync(binary).mode & 0o777);
  renameSync(replacement, sidecar);

  const plugin = await verifiedAppImagePlugin(repositoryRoot);

  rmSync(appImage, { force: true });
  rmSync(`${appImage}.sig`, { force: true });
  execFileSync(plugin, ["--appimage-extract-and-run", "--appdir", appDir], {
    env: {
      ...process.env,
      APPIMAGE_EXTRACT_AND_RUN: "1",
      ARCH: "x86_64",
      OUTPUT: appImage,
    },
    stdio: "inherit",
  });
  if (!existsSync(appImage) || !statSync(appImage).isFile()) {
    fail("AppImage output plugin did not create the package");
  }

  if (hadSignature) {
    execFileSync(
      "corepack",
      ["pnpm", "--filter", "@starweaver/desktop", "tauri", "signer", "sign", appImage],
      { cwd: repositoryRoot, env: process.env, stdio: "inherit" },
    );
    if (!existsSync(`${appImage}.sig`) || !statSync(`${appImage}.sig`).isFile()) {
      fail("repacked AppImage was not signed");
    }
  }
} finally {
  rmSync(replacement, { force: true });
}

console.log(`Finalized exact-sidecar AppImage ${appImage}`);
