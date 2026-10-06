// Builds World's installer with the fastctx it carries: the core's fastctx
// built for release the way the release workflow builds it, placed where
// Tauri takes a program to bundle beside the app, then Tauri's own build
// with the release configuration (`src-tauri/tauri.release.conf.json`),
// signed for the app's updater. Beside the installer and its signature it
// writes `latest.json`, the file installed copies read from the project's
// latest GitHub release to learn of this version.
//
//   TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… node scripts/release.mjs [notes.md]
//
// TAURI_SIGNING_PRIVATE_KEY is the signing key, or the path to it; by
// default the key kept in ~/.tauri (`atlas/repo.md`). The notes, when
// given, are what the app shows of this version. CARGO_TARGET_DIR, when
// set, is where both builds put their output; otherwise each uses its own
// default.

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildKernel } from "../../scripts/kernel.mjs";

const gui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// Where installed copies look for new versions (`plugins.updater` in
// `src-tauri/tauri.conf.json`), and where this one will be downloaded from.
const releases = "https://github.com/yc-duan/fastctx/releases";
// No variable of the session that runs this reaches the builds.
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("CLAUDE")));
env.TAURI_SIGNING_PRIVATE_KEY ??= join(homedir(), ".tauri", "world-updater.key");
if (env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === undefined) {
  throw new Error("TAURI_SIGNING_PRIVATE_KEY_PASSWORD is not set: the installer must be signed for the updater (atlas/repo.md)");
}
const notes = process.argv[2] ? readFileSync(process.argv[2], "utf8").trim() : null;
const run = (program, args, cwd) => execFileSync(program, args, { cwd, env, stdio: "inherit", shell: process.platform === "win32" });

const kernel = buildKernel(undefined, env);
const { version, productName } = JSON.parse(readFileSync(join(gui, "src-tauri", "tauri.conf.json"), "utf8"));
if (kernel.version !== version) throw new Error(`the GUI is ${version}; its kernel is ${kernel.version}`);
const triple = kernel.bundleTarget;
const exe = process.platform === "win32" ? ".exe" : "";
const binaries = join(gui, "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });
copyFileSync(kernel.binary, join(binaries, `fastctx-${triple}${exe}`));

run("npx", ["tauri", "build", "--config", join("src-tauri", "tauri.release.conf.json")], gui);

const bundle = join(env.CARGO_TARGET_DIR ?? join(gui, "src-tauri", "target"), "release", "bundle", "nsis");
const installer = `${productName}_${version}_x64-setup.exe`;
const signature = join(bundle, `${installer}.sig`);
if (!existsSync(signature)) throw new Error(`the build left no signature at ${signature}`);
// The NSIS key comes first in the updater's search; a build without the
// bundler's marker looks for the plain one, so both name the same file.
const platform = { signature: readFileSync(signature, "utf8").trim(), url: `${releases}/download/v${version}/${installer}` };
const latest = {
  version,
  ...(notes ? { notes } : {}),
  pub_date: new Date().toISOString(),
  platforms: { "windows-x86_64-nsis": platform, "windows-x86_64": platform },
};
writeFileSync(join(bundle, "latest.json"), `${JSON.stringify(latest, null, 2)}\n`);
console.log(`\n${join(bundle, installer)}\n${signature}\n${join(bundle, "latest.json")}`);
