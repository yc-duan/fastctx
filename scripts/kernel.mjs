// One release build shared by CI, local packaging, and the GUI's sidecar.
// Linux archives retain legacy GNU names while carrying static musl binaries.
//   node scripts/kernel.mjs [TARGET]
//   node scripts/kernel.mjs --tests TARGET DIR
// `--tests` builds the release tests for TARGET into DIR (the bundled ripgrep, the node's lifetime,
// job control and byte streams, and the network drill) from the very units the kernel is built of: with the kernel's
// package among theirs every dependency gets the kernel's features, and the same environment keeps
// the embedded notices, so only the tests themselves are compiled.
import { execFileSync } from "node:child_process";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { copyFileSync, existsSync, mkdirSync, renameSync } from "node:fs";

const repo = fileURLToPath(new URL("..", import.meta.url));
export const toolchain = "+1.92.0";
// The release tests, by the package each belongs to.
const tests = {
  offline: "fastctx-ripgrep", own_lifetime: "fastctx-node", job_lifecycle: "fastctx-hands", network_drill: "fastctx-hub",
  placement_drill: "fastctx-hub",
};

/** How a release kernel for `target` is compiled: its environment and its cargo. */
function release(target, environment) {
  const env = Object.fromEntries(Object.entries(environment).filter(([name]) => !name.startsWith("CLAUDE")));
  // A release build: its World is the user's, not a development one's apart.
  env.FASTCTX_RELEASE = "1";
  env.FASTCTX_THIRD_PARTY_LICENSES = resolve(repo, env.FASTCTX_THIRD_PARTY_LICENSES ?? "dist/THIRD_PARTY_LICENSES.md");
  if (!existsSync(env.FASTCTX_THIRD_PARTY_LICENSES)) {
    throw new Error("Generate dist/THIRD_PARTY_LICENSES.md with scripts/notices.py before building a release");
  }
  const host = /host: (\S+)/.exec(execFileSync("rustc", [toolchain, "-vV"], { env }).toString())[1];
  target ??= host;
  const buildTarget = target.replace(/-unknown-linux-(gnu|musl)$/, "-unknown-linux-musl");
  const linux = buildTarget.endsWith("-unknown-linux-musl");
  const windows = buildTarget.endsWith("-pc-windows-msvc");
  if (windows && !/(?:^|\s)target-feature=\+crt-static(?:\s|$)/.test(env.RUSTFLAGS ?? "")) {
    env.RUSTFLAGS = `${env.RUSTFLAGS ?? ""} -C target-feature=+crt-static`.trim();
  }
  const cargo = (argv, options) => execFileSync("cargo",
    [toolchain, linux ? "zigbuild" : "build", "--locked", "--release", ...argv, "--target", buildTarget],
    { cwd: repo, env, ...options });
  return { env, host, target, buildTarget, linux, windows, cargo };
}

export function buildKernel(target, environment = process.env) {
  const { env, host, target: bundleTarget, buildTarget, linux, windows, cargo } = release(target, environment);
  cargo(["-p", "fastctx", "--bin", "fastctx"], { stdio: "inherit" });
  const directory = resolve(repo, env.CARGO_TARGET_DIR ?? "target");
  const binary = join(directory, buildTarget, "release", windows ? "fastctx.exe" : "fastctx");
  if (linux) {
    // Nothing reads a release kernel's symbols: a panic is logged by its message and place.
    execFileSync("zig", ["objcopy", "--strip-all", binary, `${binary}.stripped`], { env, stdio: "inherit" });
    renameSync(`${binary}.stripped`, binary);
  }
  execFileSync("python", [join(repo, "scripts/package.py"), "check-binary", "--target", bundleTarget.replace(/-unknown-linux-musl$/, "-unknown-linux-gnu"),
    "--binary", binary, "--notices", env.FASTCTX_THIRD_PARTY_LICENSES], { cwd: repo, env, stdio: "inherit" });
  const metadata = JSON.parse(execFileSync("cargo", [toolchain, "metadata", "--locked", "--no-deps", "--format-version", "1"], { cwd: repo, env }));
  const version = metadata.packages.find(pkg => pkg.name === "fastctx").version;
  const architecture = buildTarget.split("-")[0];
  const crossLinux = linux && host.includes("-unknown-linux-") && architecture !== host.split("-")[0];
  const runner = crossLinux ? `qemu-${architecture}` : binary;
  const printed = execFileSync(runner, crossLinux ? [binary, "--version"] : ["--version"], { env, encoding: "utf8" });
  if (printed.replace(/\r\n/g, "\n") !== `fastctx ${version}\n`) throw new Error(`invalid kernel version output: ${printed}`);
  return { binary, buildTarget, bundleTarget, version: printed.trim().slice("fastctx ".length) };
}

/** The release tests for `target`, built after its kernel, copied into `out` by their names. */
export function buildKernelTests(target, out, environment = process.env) {
  const { windows, cargo } = release(target, environment);
  const printed = cargo(["-p", "fastctx", ...Object.entries(tests).flatMap(([name, owner]) => ["-p", owner, "--test", name]),
    "--message-format=json-render-diagnostics"], { stdio: ["ignore", "pipe", "inherit"], encoding: "utf8", maxBuffer: 1 << 28 });
  mkdirSync(out, { recursive: true });
  const built = {};
  for (const line of printed.split("\n")) {
    if (!line.startsWith("{")) continue;
    const message = JSON.parse(line);
    if (message.reason === "compiler-artifact" && message.executable && Object.hasOwn(tests, message.target.name)) {
      built[message.target.name] = join(out, message.target.name + (windows ? ".exe" : ""));
      copyFileSync(message.executable, built[message.target.name]);
    }
  }
  for (const name of Object.keys(tests)) if (!built[name]) throw new Error(`the build left no ${name} test`);
  return built;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv[2] === "--tests") console.log(JSON.stringify(buildKernelTests(process.argv[3], process.argv[4])));
  else console.log(JSON.stringify(buildKernel(process.argv[2])));
}
