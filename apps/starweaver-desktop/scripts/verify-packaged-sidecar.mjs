import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";

function fail(message) {
  console.error(`verify-packaged-sidecar: ${message}`);
  process.exit(1);
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

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function expectedPackagedDigest(expected, expectedName) {
  if (process.platform !== "darwin") return sha256(expected);
  const temporaryRoot = mkdtempSync(join(tmpdir(), "starweaver-sidecar-"));
  const signedCopy = join(temporaryRoot, expectedName);
  try {
    copyFileSync(expected, signedCopy);
    execFileSync("codesign", ["--force", "--sign", "-", "--options", "runtime", signedCopy], {
      stdio: "ignore",
    });
    return sha256(signedCopy);
  } catch {
    fail("could not reproduce the required macOS ad-hoc sidecar signature");
  } finally {
    rmSync(temporaryRoot, { force: true, recursive: true });
  }
}

const [rootArgument, expectedArgument] = process.argv.slice(2);
if (!rootArgument || !expectedArgument) {
  fail("usage: verify-packaged-sidecar.mjs <extracted-package-root> <expected-rpc-binary>");
}
const root = resolve(rootArgument);
const expected = resolve(expectedArgument);
const expectedName = basename(expected);
if (expectedName !== "starweaver-rpc" && expectedName !== "starweaver-rpc.exe") {
  fail("expected binary must use the installed sidecar name");
}
const matches = filesBelow(root).filter((path) => basename(path) === expectedName);
if (matches.length !== 1)
  fail(`expected exactly one installed ${expectedName}, found ${matches.length}`);
const [sidecar] = matches;
if (sidecar === undefined || sha256(sidecar) !== expectedPackagedDigest(expected, expectedName)) {
  fail("installed sidecar digest differs from the exact target build and packaging transform");
}
if (process.platform !== "win32" && (statSync(sidecar).mode & 0o111) === 0) {
  fail("installed Unix sidecar is not executable");
}
console.log(sidecar);
