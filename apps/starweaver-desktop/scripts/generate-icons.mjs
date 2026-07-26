import { copyFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const packageRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const corepack = process.platform === "win32" ? "corepack.cmd" : "corepack";
const generated = spawnSync(
  corepack,
  ["pnpm", "exec", "tauri", "icon", "public/app-icon.png", "--output", "src-tauri/icons"],
  { cwd: packageRoot, stdio: "inherit" },
);

if (generated.status !== 0) {
  process.exit(generated.status ?? 1);
}

for (const mobileDirectory of ["android", "ios"]) {
  rmSync(join(packageRoot, "src-tauri", "icons", mobileDirectory), {
    recursive: true,
    force: true,
  });
}
copyFileSync(
  join(packageRoot, "src-tauri", "icons", "64x64.png"),
  join(packageRoot, "public", "favicon.png"),
);
