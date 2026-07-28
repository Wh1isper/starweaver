import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

function fail(message) {
  console.error(`package-runtime-update: ${message}`);
  process.exit(1);
}

function argument(name) {
  const index = process.argv.indexOf(name);
  const value = index >= 0 ? process.argv[index + 1] : undefined;
  if (!value || value.startsWith("--")) fail(`${name} is required`);
  return value;
}

function sha256(path) {
  return `sha256:${createHash("sha256").update(readFileSync(path)).digest("hex")}`;
}

const version = argument("--version").replace(/^v/, "");
const target = argument("--target");
const revision = argument("--revision");
const binary = resolve(argument("--binary"));
const output = resolve(argument("--output"));
const supportedTargets = new Set([
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "x86_64-unknown-linux-gnu",
]);
if (!supportedTargets.has(target)) fail(`unsupported target: ${target}`);
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) fail("version must be SemVer");
if (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(revision))
  fail("revision must be a full lowercase commit digest");
if (!statSync(binary).isFile()) fail("runtime binary is missing");
const protocol = JSON.parse(
  readFileSync(resolve("protocol/host/generated/manifest.json"), "utf8"),
).protocol;
if (
  protocol?.name !== "starweaver.host" ||
  typeof protocol.major !== "number" ||
  typeof protocol.revision !== "string" ||
  typeof protocol.schemaDigest !== "string"
) {
  fail("generated protocol identity is invalid");
}
const [major, minor] = version.split(".").map(Number);
if (major === undefined || minor === undefined) fail("version range could not be derived");
const nextMinor = `${major}.${minor + 1}.0`;
const extension = target.includes("windows") ? ".exe" : "";
const assetName = `starweaver-rpc-v${version}-${target}${extension}`;
const assetPath = join(output, assetName);
mkdirSync(output, { recursive: true });
copyFileSync(binary, assetPath);
const manifestName = `starweaver-runtime-${target}.manifest.json`;
const manifest = {
  schemaVersion: 1,
  version,
  buildRevision: revision,
  rustTarget: target,
  desktopVersionRequirement: `>=${major}.${minor}.0, <${nextMinor}`,
  protocol,
  launchSchemaVersion: 1,
  storageGeneration: 1,
  asset: {
    name: assetName,
    url: `https://github.com/Wh1isper/starweaver/releases/download/v${version}/${assetName}`,
    size: statSync(assetPath).size,
    sha256: sha256(assetPath),
  },
};
const manifestPath = join(output, manifestName);
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
console.log(JSON.stringify({ assetPath, manifestPath, assetName, manifestName }));
