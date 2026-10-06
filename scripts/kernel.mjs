// One release build shared by CI, local packaging, and the GUI's sidecar.
// Linux archives retain legacy GNU names while carrying static musl binaries.
import { execFileSync } from "node:child_process";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("..", import.meta.url));
export const toolchain = "+1.92.0";

export function buildKernel(target, environment = process.env) {
  const env = Object.fromEntries(Object.entries(environment).filter(([name]) => !name.startsWith("CLAUDE")));
  const host = /host: (\S+)/.exec(execFileSync("rustc", [toolchain, "-vV"], { env }).toString())[1];
  target ??= host;
  const buildTarget = target.replace(/-unknown-linux-(gnu|musl)$/, "-unknown-linux-musl");
  const linux = buildTarget.endsWith("-unknown-linux-musl");
  const windows = buildTarget.endsWith("-pc-windows-msvc");
  if (windows && !/(?:^|\s)target-feature=\+crt-static(?:\s|$)/.test(env.RUSTFLAGS ?? "")) {
    env.RUSTFLAGS = `${env.RUSTFLAGS ?? ""} -C target-feature=+crt-static`.trim();
  }
  const command = linux ? "zigbuild" : "build";
  execFileSync("cargo", [toolchain, command, "--locked", "--release", "-p", "fastctx", "--bin", "fastctx", "--target", buildTarget],
    { cwd: repo, env, stdio: "inherit" });
  const directory = resolve(repo, env.CARGO_TARGET_DIR ?? "target");
  const binary = join(directory, buildTarget, "release", windows ? "fastctx.exe" : "fastctx");
  const metadata = JSON.parse(execFileSync("cargo", [toolchain, "metadata", "--no-deps", "--format-version", "1"], { cwd: repo, env }));
  const version = metadata.packages.find(pkg => pkg.name === "fastctx").version;
  const printed = execFileSync(binary, ["--version"], { env, encoding: "utf8" });
  if (printed.replace(/\r\n/g, "\n") !== `fastctx ${version}\n`) throw new Error(`invalid kernel version output: ${printed}`);
  return { binary, buildTarget, bundleTarget: target, version: printed.trim().slice("fastctx ".length) };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(JSON.stringify(buildKernel(process.argv[2])));
}
