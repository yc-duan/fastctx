// Builds World's installer with the fastctx it carries: the core's fastctx
// built for release the way the release workflow builds it, placed where
// Tauri takes a program to bundle beside the app, then Tauri's own build
// with the release configuration (`src-tauri/tauri.release.conf.json`).
//
//   node scripts/release.mjs
//
// CARGO_TARGET_DIR, when set, is where both builds put their output;
// otherwise each uses its own default.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const gui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(gui, "..");
// The toolchain the release workflow pins (`.github/workflows/release.yml`).
const toolchain = "+1.92.0";
// No variable of the session that runs this reaches the builds.
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("CLAUDE")));
const run = (program, args, cwd) => execFileSync(program, args, { cwd, env, stdio: "inherit", shell: process.platform === "win32" });

const triple = /host: (\S+)/.exec(execFileSync("rustc", [toolchain, "-vV"], { env }).toString())[1];
const exe = process.platform === "win32" ? ".exe" : "";
run("cargo", [toolchain, "build", "--locked", "--release", "-p", "fastctx", "--bin", "fastctx", "--target", triple], repo);

const target = env.CARGO_TARGET_DIR ?? join(repo, "target");
const binaries = join(gui, "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });
copyFileSync(join(target, triple, "release", `fastctx${exe}`), join(binaries, `fastctx-${triple}${exe}`));

run("npx", ["tauri", "build", "--config", join("src-tauri", "tauri.release.conf.json")], gui);
