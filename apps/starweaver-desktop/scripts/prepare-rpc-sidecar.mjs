import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, renameSync, rmSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SUPPORTED_TARGETS = new Set([
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "x86_64-unknown-linux-gnu",
]);

function fail(message) {
  console.error(`prepare-rpc-sidecar: ${message}`);
  process.exit(1);
}

function hostTarget() {
  try {
    return execFileSync("rustc", ["--print", "host-tuple"], { encoding: "utf8" }).trim();
  } catch {
    fail("could not determine the Rust host target");
  }
}

const release = process.argv.includes("--release");
const explicitTarget =
  process.env.STARWEAVER_DESKTOP_TARGET?.trim() || process.env.TAURI_ENV_TARGET_TRIPLE?.trim();
const target = explicitTarget || hostTarget();
if (!SUPPORTED_TARGETS.has(target)) {
  fail(`unsupported Desktop target: ${target}`);
}

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const desktopRoot = resolve(scriptDirectory, "..");
const repositoryRoot = resolve(desktopRoot, "../..");
const extension = target.includes("windows") ? ".exe" : "";
const profile = release ? "release" : "debug";
const cargoArgs = [
  "build",
  "--locked",
  "-p",
  "starweaver-rpc",
  "--bin",
  "starweaver-rpc",
  "--target",
  target,
];
if (release) cargoArgs.push("--release");

execFileSync("cargo", cargoArgs, {
  cwd: repositoryRoot,
  env: process.env,
  stdio: "inherit",
});

const source = join(repositoryRoot, "target", target, profile, `starweaver-rpc${extension}`);
const destination = join(
  desktopRoot,
  "src-tauri",
  "binaries",
  `starweaver-rpc-${target}${extension}`,
);
mkdirSync(dirname(destination), { mode: 0o700, recursive: true });
const temporary = `${destination}.${process.pid.toString()}.tmp`;
copyFileSync(source, temporary);
if (!target.includes("windows")) chmodSync(temporary, 0o500);
if (!statSync(temporary).isFile()) fail("prepared sidecar is not a regular file");
rmSync(destination, { force: true });
renameSync(temporary, destination);
console.log(`Prepared ${destination}`);
