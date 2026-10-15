// Builds World's installer for this computer's platform, carrying a checked release kernel: the one
// `--kernel` names, as the release workflow builds and tests it in a job of its own, or one built
// here. Nothing is signed for the updater here: the release's signing step signs each package bound
// to its version. A macOS application is signed ad hoc, so it runs where its owner allows it.
//   node scripts/release.mjs [--kernel PATH] [--target TRIPLE] [--out DIR]
//   node scripts/release.mjs --prepare [--target TRIPLE]
//   node scripts/release.mjs --prepared --kernel PATH [--target TRIPLE] [--out DIR]
// `--target` builds for another processor of this platform, as an Intel Mac's application is built
// on Apple silicon. `--prepare` compiles the application while its kernel is still being built,
// with stand-ins for what it carries: nothing it carries is compiled into it. `--prepared` then
// bundles that very application with what it carries, compiling nothing.
// Windows leaves its NSIS installer in DIR; macOS its disk image and the updater's archive of the
// application; Linux leaves its deb and AppImage in the bundle directory, for linux-bundle.py.
import { execFileSync } from "node:child_process";
import { closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, writeFileSync } from "node:fs";
import { availableParallelism } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildKernel, toolchain } from "../../scripts/kernel.mjs";

const gui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const option = (name) => {
  const at = args.indexOf(name);
  return at === -1 ? undefined : args[at + 1];
};
const prepare = args.includes("--prepare");
const prepared = args.includes("--prepared");
const platform = process.platform;
if (!["win32", "darwin", "linux"].includes(platform)) throw new Error("World's installers are built on Windows, macOS and Linux");
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("CLAUDE")));
env.RUSTUP_TOOLCHAIN = toolchain.slice(1);
// A release build: its World is the user's, not a development one's apart.
env.FASTCTX_RELEASE = "1";
delete env.TAURI_SIGNING_PRIVATE_KEY;
delete env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
// The Tauri CLI's own script, started by this Node: through a shell, Windows' cmd strips the quotes
// out of the JSON configuration argument.
const tauri = join(gui, "node_modules", "@tauri-apps", "cli", "tauri.js");
const run = (argv) => execFileSync(process.execPath, [tauri, ...argv], { cwd: gui, env, stdio: "inherit" });
const { version } = JSON.parse(readFileSync(join(gui, "src-tauri", "tauri.conf.json"), "utf8"));
const host = /host: (\S+)/.exec(execFileSync("rustc", [toolchain, "-vV"], { env }).toString())[1];
const target = option("--target") ?? host;
const exe = platform === "win32" ? ".exe" : "";
const binaries = join(gui, "src-tauri", "binaries");
const sidecar = join(binaries, `fastctx-${target}${exe}`);
// The release configuration's bundle, with the Linux archives a World made on a server, or a server
// added, over SSH is installed from.
const release = JSON.parse(readFileSync(join(gui, "src-tauri", "tauri.release.conf.json"), "utf8"));
const config = { bundle: { ...release.bundle, externalBin: ["binaries/fastctx"], createUpdaterArtifacts: false } };
const resources = Object.keys(config.bundle.resources ?? {}).map(resource => join(gui, "src-tauri", resource));
if (platform === "win32") config.bundle.targets = ["nsis"];
if (platform === "darwin") {
  config.bundle.targets = ["app", "dmg"];
  config.bundle.macOS = { signingIdentity: "-" };
}
if (platform === "linux") {
  config.bundle.targets = ["deb", "appimage"];
  // Debian keeps a package's notices under its name, which Tauri makes the product's in kebab case.
  config.bundle.linux = { deb: { depends: ["pkexec"], files: {
    "/usr/share/doc/fastctx/copyright": "licenses/copyright",
    "/usr/share/doc/fastctx/LICENSE-APACHE": "licenses/LICENSE-APACHE",
    "/usr/share/doc/fastctx/NOTICE": "licenses/NOTICE",
    "/usr/share/doc/fastctx/THIRD_PARTY_LICENSES.md": "licenses/THIRD_PARTY_LICENSES.md",
  } } };
}
const crossing = target === host ? [] : ["--target", target];
const build = (bundling) => run([
  "build", ...crossing, ...(bundling ? [] : ["--no-bundle"]),
  "--config", JSON.stringify(config), "--", "--locked", "-j", env.CARGO_BUILD_JOBS ?? String(availableParallelism()),
]);
// A sidecar or resource that changes reruns tauri-build, which then compiles the application again,
// though it compiles nothing of them in: the prepared application is bundled as it is.
const bundlePrepared = () => run(["bundle", ...crossing, "--config", JSON.stringify(config)]);
// What a stand-in holds, so that nothing built with one can pass for a package.
const STAND_IN = "a stand-in while the release kernel is built\n";
const standsIn = (path) => {
  const descriptor = openSync(path, "r");
  const start = Buffer.alloc(STAND_IN.length);
  readSync(descriptor, start, 0, start.length, 0);
  closeSync(descriptor);
  return start.toString() === STAND_IN;
};

if (prepare) {
  mkdirSync(binaries, { recursive: true });
  for (const path of [sidecar, ...resources]) {
    mkdirSync(dirname(path), { recursive: true });
    if (!existsSync(path)) writeFileSync(path, STAND_IN);
  }
  build(false);
  process.exit(0);
}

const kernelPath = option("--kernel");
let kernel;
if (kernelPath) {
  const printed = execFileSync(kernelPath, ["--version"], { env, encoding: "utf8" }).replace(/\r\n/g, "\n");
  if (printed !== `fastctx ${version}\n`) throw new Error(`the kernel at ${kernelPath} says ${printed.trim()}, not fastctx ${version}`);
  kernel = { binary: kernelPath, version };
} else {
  kernel = buildKernel(target === host ? undefined : target, env);
}
if (kernel.version !== version) throw new Error(`the GUI is ${version}; its kernel is ${kernel.version}`);
const guiMetadata = JSON.parse(execFileSync("cargo", [toolchain, "metadata", "--locked", "--manifest-path", join(gui, "src-tauri", "Cargo.toml"), "--no-deps", "--format-version", "1"], { env }));
if (guiMetadata.packages.find(pkg => pkg.name === "fastctx-gui").version !== version || JSON.parse(readFileSync(join(gui, "package.json"), "utf8")).version !== version) {
  throw new Error("the native GUI, frontend and installer versions differ");
}
mkdirSync(binaries, { recursive: true });
copyFileSync(kernel.binary, sidecar);
for (const resource of resources) {
  if (!existsSync(resource) || standsIn(resource)) throw new Error(`the installer carries ${resource}, which is not there`);
}
if (prepared) bundlePrepared();
else build(true);
const bundle = resolve(gui, "src-tauri", env.CARGO_TARGET_DIR ?? "target", ...(target === host ? [] : [target]), "release", "bundle");
const out = resolve(option("--out") ?? join(gui, "..", "dist", "gui"));
mkdirSync(out, { recursive: true });
const one = (directory, suffix) => {
  const found = readdirSync(directory).filter(name => name.endsWith(suffix));
  if (found.length !== 1) throw new Error(`expected one ${suffix} in ${directory}: ${found}`);
  return found[0];
};
if (platform === "win32") {
  const installer = one(join(bundle, "nsis"), "-setup.exe");
  copyFileSync(join(bundle, "nsis", installer), join(out, installer));
  console.log(join(out, installer));
}
if (platform === "darwin") {
  const image = one(join(bundle, "dmg"), ".dmg");
  copyFileSync(join(bundle, "dmg", image), join(out, image));
  // The updater takes the whole application as a gzipped tar of its bundle, as Tauri makes it.
  const arch = target.startsWith("aarch64") ? "aarch64" : "x64";
  const archive = join(out, `Fastctx_${version}_${arch}.app.tar.gz`);
  execFileSync("tar", ["-czf", archive, "-C", join(bundle, "macos"), "Fastctx.app"], { env, stdio: "inherit" });
  console.log(join(out, image));
  console.log(archive);
}
if (platform === "linux") console.log(bundle);
