import { readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

function fail(message) {
  console.error(`finalize-update-metadata: ${message}`);
  process.exit(1);
}

function argument(name) {
  const index = process.argv.indexOf(name);
  const value = index >= 0 ? process.argv[index + 1] : undefined;
  if (!value || value.startsWith("--")) fail(`${name} is required`);
  return value;
}

const version = argument("--version").replace(/^v/, "");
const publishedAt = argument("--published-at");
const output = resolve(argument("--output"));
const metadataFiles = readdirSync(output).filter(
  (name) => name.startsWith("desktop-update-") && name.endsWith(".json"),
);
if (metadataFiles.length !== 4)
  fail(`expected four Desktop native-target records, found ${metadataFiles.length}`);
const platforms = {};
for (const name of metadataFiles) {
  const metadata = JSON.parse(readFileSync(join(output, name), "utf8"));
  if (!Array.isArray(metadata.entries) || metadata.entries.length === 0) {
    fail(`invalid updater metadata: ${name}`);
  }
  for (const entry of metadata.entries) {
    if (
      typeof entry.platform !== "string" ||
      typeof entry.url !== "string" ||
      typeof entry.signature !== "string" ||
      entry.signature.length === 0 ||
      entry.signature.length > 16 * 1024 ||
      platforms[entry.platform] !== undefined
    ) {
      fail(`invalid or duplicate updater entry: ${name}`);
    }
    platforms[entry.platform] = { url: entry.url, signature: entry.signature };
  }
  rmSync(join(output, name));
}
const required = [
  "darwin-aarch64",
  "darwin-x86_64",
  "linux-x86_64-appimage",
  "linux-x86_64-deb",
  "windows-x86_64",
];
if (required.some((platform) => platforms[platform] === undefined)) {
  fail("Desktop updater metadata does not cover the reviewed platform matrix");
}
writeFileSync(
  join(output, "latest.json"),
  `${JSON.stringify(
    {
      version,
      notes: `Starweaver Desktop v${version}`,
      pub_date: publishedAt,
      platforms,
    },
    null,
    2,
  )}\n`,
);
