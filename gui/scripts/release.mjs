// Builds an installer carrying the matching, checked release kernel.
// --unsigned omits updater artifacts and never selects a signing key.
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildKernel, toolchain } from "../../scripts/kernel.mjs";

const gui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const releases = "https://github.com/yc-duan/fastctx/releases";
const args = process.argv.slice(2);
const unsigned = args.includes("--unsigned");
const notesFile = args.filter(arg => arg !== "--unsigned");
if (notesFile.length > 1) throw new Error("Usage: node scripts/release.mjs [--unsigned] [notes.md]");
const linux = process.platform === "linux";
if (!linux && process.platform !== "win32") throw new Error("Fastctx release packaging supports Windows and Linux");
if (linux && !unsigned) throw new Error("Linux packages must finish native notices and verification with scripts/release-linux.sh; updater signing is a separate step");
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("CLAUDE")));
env.RUSTUP_TOOLCHAIN = toolchain.slice(1);
if (unsigned) {
  delete env.TAURI_SIGNING_PRIVATE_KEY;
  delete env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
} else {
  env.TAURI_SIGNING_PRIVATE_KEY ??= join(homedir(), ".tauri", "world-updater.key");
  if (env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === undefined) throw new Error("TAURI_SIGNING_PRIVATE_KEY_PASSWORD is required for signed updater artifacts");
}
const notes = notesFile[0] ? readFileSync(notesFile[0], "utf8").trim() : null;
// The Tauri CLI's own script, started by this Node: through a shell, Windows' cmd strips the quotes
// out of the JSON configuration argument.
const tauri = join(gui, "node_modules", "@tauri-apps", "cli", "tauri.js");
const run = (argv, cwd) => execFileSync(process.execPath, [tauri, ...argv], { cwd, env, stdio: "inherit" });
const kernel = buildKernel(undefined, env);
const { version } = JSON.parse(readFileSync(join(gui, "src-tauri", "tauri.conf.json"), "utf8"));
if (kernel.version !== version) throw new Error(`the GUI is ${version}; its kernel is ${kernel.version}`);
const guiMetadata = JSON.parse(execFileSync("cargo", [toolchain, "metadata", "--locked", "--manifest-path", join(gui, "src-tauri", "Cargo.toml"), "--no-deps", "--format-version", "1"], { env }));
if (guiMetadata.packages.find(pkg => pkg.name === "fastctx-gui").version !== version || JSON.parse(readFileSync(join(gui, "package.json"), "utf8")).version !== version) {
  throw new Error("the native GUI, frontend and installer versions differ");
}
const triple = kernel.bundleTarget;
const binaries = join(gui, "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });
copyFileSync(kernel.binary, join(binaries, `fastctx-${triple}${linux ? "" : ".exe"}`));
// The release configuration's bundle, with the Linux archives a World made on a server, or a server
// added, over SSH is installed from; this build sets whether updater artifacts are made.
const release = JSON.parse(readFileSync(join(gui, "src-tauri", "tauri.release.conf.json"), "utf8"));
const config = { bundle: { ...release.bundle, externalBin: ["binaries/fastctx"], createUpdaterArtifacts: !unsigned } };
if (linux) {
  config.bundle.targets = ["deb", "appimage"];
  // Debian keeps a package's notices under its name, which Tauri makes the product's in kebab case.
  config.bundle.linux = { deb: { depends: ["pkexec"], files: {
    "/usr/share/doc/fastctx/copyright": "licenses/copyright",
    "/usr/share/doc/fastctx/LICENSE-APACHE": "licenses/LICENSE-APACHE",
    "/usr/share/doc/fastctx/NOTICE": "licenses/NOTICE",
    "/usr/share/doc/fastctx/THIRD_PARTY_LICENSES.md": "licenses/THIRD_PARTY_LICENSES.md",
  } } };
}
run(["build", ...(unsigned ? ["--no-sign"] : []), "--config", JSON.stringify(config), "--", "--locked", "-j", env.CARGO_BUILD_JOBS ?? "4"], gui);
const base = resolve(gui, "src-tauri", env.CARGO_TARGET_DIR ?? "target", "release", "bundle");
const directory = join(base, linux ? "appimage" : "nsis");
const extension = linux ? ".AppImage" : "-setup.exe";
const installers = readdirSync(directory).filter(name => name.endsWith(extension));
if (installers.length !== 1) throw new Error(`expected one installer in ${directory}: ${installers}`);
const installer = installers[0];
if (unsigned) {
  console.log(`Unsigned local installer: ${join(directory, installer)}`);
} else {
  const signature = join(directory, `${installer}.sig`);
  if (!existsSync(signature)) throw new Error(`the build left no signature at ${signature}`);
  const platform = { signature: readFileSync(signature, "utf8").trim(), url: `${releases}/download/v${version}/${installer}` };
  const arch = process.arch === "x64" ? "x86_64" : "aarch64";
  const keys = linux ? [`linux-${arch}-appimage`, `linux-${arch}`] : ["windows-x86_64-nsis", "windows-x86_64"];
  writeFileSync(join(directory, "latest.json"), `${JSON.stringify({ version, ...(notes ? { notes } : {}), pub_date: new Date().toISOString(), platforms: Object.fromEntries(keys.map(key => [key, platform])) }, null, 2)}\n`);
  console.log(join(directory, installer));
}
