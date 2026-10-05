import { readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const require = createRequire(import.meta.url);
const directory = dirname(require.resolve("pdfjs-dist/package.json"));
const prefix = "virtual:world-pdf/";

/** Binary resources load as same-origin modules: the shell deliberately
 * allows scripts but does not allow fetch to its own asset origin. */
export function pdfAssets() {
  let used = false;
  const files = new Map();
  for (const [folder, extension] of [["cmaps", ".bcmap"], ["standard_fonts", null]]) {
    for (const name of readdirSync(join(directory, folder))) {
      if (extension ? name.endsWith(extension) : /\.(pfb|ttf)$/.test(name)) {
        files.set(`${folder}/${name}`, join(directory, folder, name));
      }
    }
  }
  return {
    name: "world-pdf-assets",
    buildStart() { used = false; },
    resolveId(id) {
      if (id === prefix + "resources" || files.has(id.slice(prefix.length)) && id.startsWith(prefix)) return "\0" + id;
    },
    load(id) {
      if (id === "\0" + prefix + "resources") {
        used = true;
        return `export default {${[...files.keys()].map((name) => `${JSON.stringify(name)}: () => import(${JSON.stringify(prefix + name)})`).join(",")}};`;
      }
      if (!id.startsWith("\0" + prefix)) return;
      const file = files.get(id.slice(prefix.length + 1));
      if (file) return `export default ${JSON.stringify(readFileSync(file).toString("base64"))};`;
    },
    generateBundle() {
      if (!used) return;
      // This filename is part of PDF.js's decoder contract, not a URL from a PDF.
      for (const name of ["openjpeg_nowasm_fallback.js", "LICENSE_OPENJPEG", "LICENSE_PDFJS_OPENJPEG"]) {
        this.emitFile({ type: "asset", fileName: `pdfjs/${name}`, source: readFileSync(join(directory, "wasm", name)) });
      }
      this.emitFile({ type: "asset", fileName: "pdfjs/LICENSE", source: readFileSync(join(directory, "LICENSE")) });
      for (const folder of ["cmaps", "standard_fonts"]) {
        for (const name of readdirSync(join(directory, folder)).filter((name) => name.startsWith("LICENSE"))) {
          this.emitFile({ type: "asset", fileName: `pdfjs/${folder}/${name}`, source: readFileSync(join(directory, folder, name)) });
        }
      }
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        if (request.url?.split("?")[0] !== `${server.config.base}pdfjs/openjpeg_nowasm_fallback.js`) return next();
        response.setHeader("Content-Type", "text/javascript");
        response.end(readFileSync(join(directory, "wasm", "openjpeg_nowasm_fallback.js")));
      });
    },
  };
}
