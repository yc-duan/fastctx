// Legal material follows the packages actually read by the frontend build,
// including the separately bundled page runtime and Vite's injected helpers.
import { existsSync, readFileSync, readdirSync, realpathSync } from "node:fs";
import { isAbsolute, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const gui = fileURLToPath(new URL("..", import.meta.url));
const lock = JSON.parse(readFileSync(join(gui, "package-lock.json"), "utf8"));
const overrides = {
  "@tauri-apps/plugin-opener@2.7.0": ["tauri-plugin-opener-2.7.0-MIT.txt"],
  "@tauri-apps/plugin-updater@2.13.1": ["tauri-plugin-updater-2.13.1-MIT.txt"],
  "qrcode-generator@2.0.4": ["qrcode-generator-2.0.4-MIT.txt"],
};

// A worktree may borrow another checkout's packages through a link, and
// Vite names their files by the real path: a package's place in the lock is
// read against the real `node_modules`.
const modules = realpathSync(join(gui, "node_modules"));
// macOS and Windows ignore case, and a tool reading files there may name them
// in another case than the disk's, as TypeScript lowers whole paths on a Mac.
const ignoresCase = process.platform === "darwin" || process.platform === "win32";
function locationOf(directory) {
  let inside = relative(modules, directory);
  const outside = (path) => path.startsWith("..") || isAbsolute(path);
  if (outside(inside) && ignoresCase) {
    const folded = relative(modules.toLowerCase(), directory.toLowerCase());
    if (!outside(folded)) inside = directory.slice(directory.length - folded.length);
  }
  const location = outside(inside) ? relative(gui, directory) : join("node_modules", inside);
  return location.replaceAll("\\", "/");
}

function packageDirectory(input) {
  const name = input.replace(/^\0/, "").replaceAll("\\", "/").split("?")[0];
  const offset = name.lastIndexOf("/node_modules/");
  if (offset === -1) return null;
  const parts = name.slice(offset + "/node_modules/".length).split("/");
  return name.slice(0, offset) + "/node_modules/" + parts.slice(0, parts[0].startsWith("@") ? 2 : 1).join("/");
}

function section(name, text) {
  const fence = "`".repeat(Math.max(3, ...[...text.matchAll(/`+/g)].map(([run]) => run.length + 1)));
  return `\n## ${name}\n\n${fence}text\n${text}\n${fence}\n`;
}

export function frontendLicenses() {
  return {
    name: "world-frontend-licenses",
    generateBundle() {
      const paths = new Set([join(gui, "node_modules/vite")]);
      for (const input of [...this.getModuleIds(), ...this.getWatchFiles()]) {
        const directory = packageDirectory(input);
        if (directory) paths.add(directory);
      }
      let text = "# Frontend third-party notices\n\nPackage licenses and copyrights from this build's input files. " +
        "The Vite entry covers its injected preload helpers. Additional embedded components and fonts follow.\n";
      const inventory = [];
      const seen = new Set();
      for (const directory of [...paths].sort()) {
        const metadata = JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
        const key = `${metadata.name}@${metadata.version}`;
        const location = locationOf(directory);
        if (lock.packages[location]?.version !== metadata.version) throw new Error(`Unpinned license input: ${location}`);
        if (seen.has(key)) continue;
        seen.add(key);
        const files = readdirSync(directory).filter(name => /^(licen[cs]e|copying|notice|copyright)(?:$|[-_.])/i.test(name) &&
          !/\.(?:[cm]?js|ts)$/i.test(name));
        const extra = overrides[key] ?? [];
        if (!files.some(name => !name.endsWith(".spdx")) && !extra.length) throw new Error(`No complete license for ${key}`);
        text += `\n# ${key}\n\nSource: ${lock.packages[location].resolved}\n`;
        for (const name of files.sort()) text += section(name, readFileSync(join(directory, name), "utf8"));
        for (const name of extra) text += section(name, readFileSync(join(gui, "licenses", name), "utf8"));
        inventory.push({ name: metadata.name, version: metadata.version, location, files, supplemental: extra });
      }
      if (seen.has("pdfjs-dist@5.4.624")) {
        const source = readFileSync(join(gui, "node_modules/pdfjs-dist/legacy/build/pdf.mjs"), "utf8");
        if (!/version: ['"]3\.47\.0['"]/.test(source)) throw new Error("Re-audit PDF.js's embedded core-js version");
        text += section("PDF.js embedded core-js 3.47.0 (MIT)", readFileSync(join(gui, "licenses/core-js-3.47.0-MIT.txt"), "utf8"));
      } else if ([...seen].some(key => key.startsWith("pdfjs-dist@"))) {
        throw new Error("Re-audit embedded PDF.js component licenses after updating pdfjs-dist");
      }
      if (seen.has("katex@0.16.22")) {
        text += section("KaTeX fonts (SIL OFL 1.1)", readFileSync(join(gui, "licenses/katex-0.16.22-fonts-OFL.txt"), "utf8"));
      } else if ([...seen].some(key => key.startsWith("katex@"))) {
        throw new Error("Re-audit KaTeX font licenses after updating katex");
      }
      text += section("Interface font attributions", readFileSync(join(gui, "public/fonts/NOTICE.txt"), "utf8"));
      for (const directory of readdirSync(join(gui, "public/fonts")).sort()) {
        const license = join(gui, "public/fonts", directory, "OFL.txt");
        if (existsSync(license)) text += section(`${directory} OFL`, readFileSync(license, "utf8"));
      }
      this.emitFile({ type: "asset", fileName: "THIRD_PARTY_FRONTEND.md", source: text });
      this.emitFile({ type: "asset", fileName: "licenses/frontend-packages.json", source: JSON.stringify(inventory, null, 2) + "\n" });
    },
  };
}
