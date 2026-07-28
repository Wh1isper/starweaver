import {
  copyFileSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";

function fail(message) {
  console.error(`collect-desktop-artifacts: ${message}`);
  process.exit(1);
}

function argument(name) {
  const index = process.argv.indexOf(name);
  const value = index >= 0 ? process.argv[index + 1] : undefined;
  if (!value || value.startsWith("--")) fail(`${name} is required`);
  return value;
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

function one(files, predicate, label) {
  const matches = files.filter(predicate);
  if (matches.length !== 1) fail(`expected one ${label}, found ${matches.length}`);
  return matches[0];
}

function copy(source, destinationName, output) {
  if (source === undefined) fail(`missing source for ${destinationName}`);
  const destination = join(output, destinationName);
  copyFileSync(source, destination);
  if (!statSync(destination).isFile()) fail(`failed to collect ${destinationName}`);
  return destination;
}

const version = argument("--version").replace(/^v/, "");
const target = argument("--target");
const bundleRoot = resolve(argument("--bundle-root"));
const output = resolve(argument("--output"));
const files = filesBelow(bundleRoot);
mkdirSync(output, { recursive: true });

const installers = [];
const updaters = [];
if (target === "x86_64-unknown-linux-gnu") {
  const appImageSource = one(files, (path) => path.endsWith(".AppImage"), "AppImage");
  const appImageName = `starweaver-desktop-v${version}-${target}.AppImage`;
  const debSource = one(files, (path) => path.endsWith(".deb"), "deb package");
  const debName = `starweaver-desktop-v${version}-${target}.deb`;
  installers.push([appImageSource, appImageName], [debSource, debName]);
  updaters.push(
    ["linux-x86_64-appimage", appImageSource, appImageName],
    ["linux-x86_64-deb", debSource, debName],
  );
} else if (target === "x86_64-apple-darwin" || target === "aarch64-apple-darwin") {
  const platform = target.startsWith("aarch64") ? "darwin-aarch64" : "darwin-x86_64";
  const updaterSource = one(files, (path) => path.endsWith(".app.tar.gz"), "macOS updater archive");
  const updaterName = `starweaver-desktop-v${version}-${target}.app.tar.gz`;
  installers.push([
    one(files, (path) => path.endsWith(".dmg"), "DMG"),
    `starweaver-desktop-v${version}-${target}.dmg`,
  ]);
  installers.push([updaterSource, updaterName]);
  updaters.push([platform, updaterSource, updaterName]);
} else if (target === "x86_64-pc-windows-msvc") {
  const updaterSource = one(
    files,
    (path) => path.toLowerCase().endsWith("-setup.exe"),
    "NSIS installer",
  );
  const updaterName = `starweaver-desktop-v${version}-${target}-setup.exe`;
  installers.push([updaterSource, updaterName]);
  updaters.push(["windows-x86_64", updaterSource, updaterName]);
} else {
  fail(`unsupported target: ${target}`);
}

for (const [source, name] of installers) copy(source, name, output);
const entries = [];
for (const [platform, updaterSource, updaterName] of updaters) {
  const signatureSource = one(
    files,
    (path) => path === `${updaterSource}.sig`,
    `${basename(updaterSource)} signature`,
  );
  copy(signatureSource, `${updaterName}.sig`, output);
  const signature = readFileSync(signatureSource, "utf8").trim();
  if (signature.length === 0 || signature.length > 16 * 1024) {
    fail("updater signature size is invalid");
  }
  entries.push({
    platform,
    url: `https://github.com/Wh1isper/starweaver/releases/download/v${version}/${updaterName}`,
    signature,
  });
}
writeFileSync(
  join(output, `desktop-update-${target}.json`),
  `${JSON.stringify({ entries }, null, 2)}\n`,
);
