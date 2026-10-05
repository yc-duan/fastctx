// The page runtime as one plain script, for the interface to put at the
// head of every page it opens: `import runtime from
// "virtual:world-page-runtime"` gives its text. It is built apart from the
// interface because it runs in another document, before anything else
// there, where a module that waits for its imports would come too late.

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const ID = "virtual:world-page-runtime";
const RESOLVED = `\0${ID}`;
const entry = fileURLToPath(new URL("../src/page/runtime.ts", import.meta.url));

/** Vite's `?raw`: a file's text as the module's default export. */
const raw = {
  name: "raw",
  setup(esbuild) {
    esbuild.onResolve({ filter: /\?raw$/ }, (args) => ({
      path: fileURLToPath(new URL(args.path.slice(0, -4), `file:///${args.resolveDir.replace(/\\/g, "/")}/`)),
      namespace: "raw",
    }));
    esbuild.onLoad({ filter: /.*/, namespace: "raw" }, async (args) => ({
      contents: `export default ${JSON.stringify(await readFile(args.path, "utf8"))};`,
      loader: "js",
      watchFiles: [args.path],
    }));
  },
};

export async function bundle() {
  const built = await build({
    entryPoints: [entry],
    bundle: true,
    format: "iife",
    minify: true,
    write: false,
    metafile: true,
    // As the interface: a phone's WebView can be years behind.
    target: ["es2020", "chrome80"],
    plugins: [raw],
    logLevel: "silent",
  });
  // The text goes inside a <script> element: nothing in it may end one.
  const code = built.outputFiles[0].text.replace(/<\/script/gi, "<\\/script");
  return { code, inputs: Object.keys(built.metafile.inputs) };
}

export function pageRuntime() {
  /** The files the last build read, as the watcher names them. */
  let inputs = new Set();
  const key = (file) => resolve(file).toLowerCase();
  return {
    name: "world-page-runtime",
    resolveId(id) {
      return id === ID ? RESOLVED : null;
    },
    async load(id) {
      if (id !== RESOLVED) return null;
      const built = await bundle();
      inputs = new Set(built.inputs.map((input) => key(input.startsWith("raw:") ? input.slice(4) : input)));
      for (const input of inputs) this.addWatchFile(input);
      return `export default ${JSON.stringify(built.code)};`;
    },
    // Nothing in the interface imports the runtime's files, so a change to
    // one moves nothing by itself: the built text is dropped and the
    // interface loads again with a new one.
    configureServer(server) {
      server.watcher.on("change", (file) => {
        if (!inputs.has(key(file))) return;
        const built = server.moduleGraph.getModuleById(RESOLVED);
        if (built) server.moduleGraph.invalidateModule(built);
        server.ws.send({ type: "full-reload" });
      });
    },
  };
}
