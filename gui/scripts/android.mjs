import { spawnSync } from "node:child_process";
import { cpSync, existsSync, readFileSync, writeFileSync, readdirSync, openSync, closeSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

// The generated project contains SDK/cache paths. Keep the native sources
// separately and apply them after each init, including on a clean checkout.
const gui = fileURLToPath(new URL("../", import.meta.url));
const cli = path.join(gui, "node_modules/@tauri-apps/cli/tauri.js");
const generated = path.join(gui, "src-tauri/gen/android");
// Each release installs over the one before it: Android takes an APK only with a higher version
// code, so the code orders releases as their versions do, a test release below its own stable one.
const { version } = JSON.parse(readFileSync(path.join(gui, "src-tauri/tauri.conf.json"), "utf8"));
const [, major, minor, patch, beta] = /^(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$/.exec(version) ?? [];
if (major === undefined || Number(beta ?? 0) > 98) throw new Error(`no Android version code for ${version}`);
const versionCode = ((Number(major) * 1000 + Number(minor)) * 1000 + Number(patch)) * 100 + (beta === undefined ? 99 : Number(beta));
// The CLI sets TAURI_CONFIG for the build itself, from the configuration it is given: the code goes
// in as configuration of the build.
const versioned = JSON.stringify({ bundle: { android: { versionCode } } });
// The library a phone loads keeps only the symbols it is loaded by: its symbol table is a third of it.
const env = { ...process.env, CARGO_PROFILE_RELEASE_STRIP: "symbols" };
function tauri(args) {
  const result = spawnSync(process.execPath, [cli, "android", ...args], { cwd: gui, stdio: "inherit", env });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
if (!existsSync(path.join(generated, "app/build.gradle.kts"))) tauri(["init", "--ci", "--skip-targets-install"]);
// Cargo may have cached tauri's build script in another worktree, or from
// an Android check without codegen variables. Materialize its exact locked
// templates here as well; no dependency-cache mutation is needed.
const metadataFile = path.join(generated, "world-cargo-metadata.json");
const metadataOutput = openSync(metadataFile, "w");
let metadataResult;
try {
  metadataResult = spawnSync("cargo", ["metadata", "--locked", "--format-version", "1", "--filter-platform", "aarch64-linux-android", "--manifest-path", path.join(gui, "src-tauri/Cargo.toml")],
    { cwd: gui, stdio: ["ignore", metadataOutput, "inherit"], windowsHide: true });
} finally { closeSync(metadataOutput); }
if (metadataResult.error) throw metadataResult.error;
if (metadataResult.status !== 0) process.exit(metadataResult.status ?? 1);
const metadata = JSON.parse(readFileSync(metadataFile, "utf8"));
const tauriPackage = metadata.packages.find((pkg) => pkg.name === "tauri");
if (!tauriPackage) throw new Error("The shell's locked dependency graph has no tauri package");
const tauriSource = path.dirname(tauriPackage.manifest_path);
const kotlin = path.join(generated, "app/src/main/java/dev/fastctx/world/generated");
mkdirSync(kotlin, { recursive: true });
function writeChanged(file, content) {
  if (!existsSync(file) || readFileSync(file, "utf8") !== content) writeFileSync(file, content);
}
for (const name of readdirSync(path.join(tauriSource, "mobile/android-codegen"))) {
  const content = readFileSync(path.join(tauriSource, "mobile/android-codegen", name), "utf8")
    .replaceAll("{{package}}", "dev.fastctx.world").replaceAll("{{library}}", "fastctx_gui_lib");
  writeChanged(path.join(kotlin, name), content);
}
writeChanged(path.join(generated, "app/proguard-tauri.pro"),
  readFileSync(path.join(tauriSource, "mobile/proguard-tauri.pro"), "utf8").replaceAll("$PACKAGE", "dev.fastctx.world"));
// The CLI guesses a package manager at init time. A direct Node invocation
// otherwise leaves Gradle trying to execute `node tauri`. Use the actual
// installed CLI, and keep its failure instead of trying executable suffixes.
const tasks = readdirSync(path.join(generated, "buildSrc/src"), { recursive: true })
  .filter((name) => name.endsWith("BuildTask.kt"));
if (tasks.length !== 1) throw new Error("Expected one Tauri Android BuildTask");
const taskFile = path.join(generated, "buildSrc/src", tasks[0]);
const originalTask = readFileSync(taskFile, "utf8");
let task = originalTask.replace(/fun assemble\(\) \{[\s\S]*?\n    fun runTauriCli/, `fun assemble() {\n        runTauriCli("""${process.execPath}""")\n    }\n\n    fun runTauriCli`);
task = task.replace(/val args = listOf\([^\n]*\);/, `val args = listOf("""${cli}""", "android", "android-studio-script");`);
if (!task.includes(cli)) throw new Error("Tauri Android build task template changed");
if (task !== originalTask) writeFileSync(taskFile, task);
cpSync(path.join(gui, "src-tauri/android"), path.join(generated, "app/src/main"), { recursive: true });
const gradle = path.join(generated, "app/build.gradle.kts");
let build = readFileSync(gradle, "utf8");
const dependency = '    implementation("com.journeyapps:zxing-android-embedded:4.3.0")';
if (!build.includes(dependency)) {
  if (!build.includes("dependencies {")) throw new Error("Tauri Android template has no dependencies block");
  build = build.replace("dependencies {", `dependencies {\n${dependency}`);
  writeFileSync(gradle, build);
}
// One APK carries a library for each of four processors, and a phone unpacks only its own: stored
// compressed, the download is less than half, and what stays on the phone is smaller too.
const compressed = "            packaging { jniLibs { useLegacyPackaging = true } }";
if (!build.includes(compressed)) {
  if (!build.includes('        getByName("release") {')) throw new Error("Tauri Android template has no release build type");
  build = build.replace('        getByName("release") {', `        getByName("release") {\n${compressed}`);
  writeFileSync(gradle, build);
}
const args = process.argv.slice(2);
if (args[0] !== "prepare") {
  const [command, ...rest] = args.length ? args : ["build", "--apk"];
  tauri(command === "build" ? [command, "--config", versioned, ...rest] : [command, ...rest]);
}
