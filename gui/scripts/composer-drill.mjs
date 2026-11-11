// Focused GUI counterexamples with explicit protocol stubs. The browser supplies
// real IndexedDB, Web Locks, and IME DOM; no account or model is used.
import { build } from "esbuild";
import { execFileSync, spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const gui = fileURLToPath(new URL("../", import.meta.url));
const args = process.argv.slice(2);
const option = (name, fallback) => { const at = args.indexOf(name); return at === -1 ? fallback : args[at + 1]; };
const source = path.resolve(option("--source", gui));
const scratch = option("--scratch", process.env.FASTCTX_SCRATCH);
if (!scratch) throw new Error("pass --scratch with an isolated absolute directory");
const bundle = async (contents, revision) => {
  const result = await build({
    stdin: { contents, resolveDir: source, loader: "ts" },
    bundle: true, write: false, format: "esm", platform: "browser", nodePaths: [path.join(gui, "node_modules")], define: { "import.meta.env": "{}" }, logLevel: "silent",
    plugins: revision ? [{ name: "frozen-composer", setup(builder) {
      builder.onLoad({ filter: /\.[cm]?[jt]s$/ }, (args) => {
        const relative = path.relative(source, args.path).replace(/\\/g, "/");
        if (!relative.startsWith("src/")) return;
        return { contents: execFileSync("git", ["-C", path.dirname(source), "show", `${revision}:gui/${relative}`], { encoding: "utf8" }), loader: relative.endsWith(".ts") ? "ts" : "js", resolveDir: path.dirname(args.path) };
      });
    } }] : [],
  });
  return `data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}`;
};
const moduleURL = await bundle('export { NativeComposer,encode,decode } from "./src/conversation/composer/native"; export { ComposerDocument } from "./src/conversation/composer/document"; export { ComposerSession } from "./src/conversation/composer/session"; export { NativeComposerDocument } from "./src/conversation/composer/native-document"; export { composerJournalStore } from "./src/conversation/composer/journal"; export { bindComposer } from "./src/conversation/composer/textarea"; export * as Y from "yjs";', option("--revision", null));
const legacyURL = await bundle('export { ComposerSession } from "./src/conversation/composer/session"; export { composerJournalStore } from "./src/conversation/composer/journal"; export { bindComposer } from "./src/conversation/composer/textarea";', "6a794c86");
const fixtures = await build({ entryPoints: [path.join(gui, "scripts/composer-drill/cases.mjs")], bundle: true, write: false, format: "esm", platform: "browser" });
const fixtureURL = `data:text/javascript;base64,${Buffer.from(fixtures.outputFiles[0].text).toString("base64")}`;
const server = createServer((_, reply) => { reply.writeHead(200, { "Content-Type": "text/html" }); reply.end("<!doctype html><html><body></body></html>"); });
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const address = server.address();
const profile = await mkdtemp(path.join(scratch, "composer-drill-"));
const port = 10000 + process.pid % 10000;
const chrome = spawn(option("--browser", "C:/Program Files/Google/Chrome/Application/chrome.exe"), ["--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-first-run", "about:blank"], { stdio: "ignore" });
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let socket;
try {
  let targets;
  for (let attempt = 0; attempt < 100; attempt++) { try { targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json(); break; } catch { await delay(100); } }
  if (!targets) throw new Error("headless browser did not open its debugging endpoint");
  socket = new WebSocket(targets.find((target) => target.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve) => socket.addEventListener("open", resolve, { once: true }));
  let serial = 0; const waiting = new Map();
  socket.addEventListener("message", (event) => { const message = JSON.parse(event.data); if (!message.id) return; const call = waiting.get(message.id); waiting.delete(message.id); message.error ? call.reject(message.error) : call.resolve(message.result); });
  const send = (method, params = {}) => new Promise((resolve, reject) => { waiting.set(++serial, { resolve, reject }); socket.send(JSON.stringify({ id: serial, method, params })); });
  await send("Page.enable"); await send("Runtime.enable"); await send("Page.navigate", { url: `http://127.0.0.1:${address.port}/` }); await delay(50);
  const selected = option("--cases", "A4,A2,A6,A7,A8,A12,C1").split(",");
  const expression = `(async()=>{const {run}=await import(${JSON.stringify(fixtureURL)});return run(${JSON.stringify(moduleURL)},${JSON.stringify(selected)},${Number(option("--seed", "1"))},${JSON.stringify(legacyURL)});})()`;
  const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error((result.exceptionDetails.exception?.description ?? result.exceptionDetails.text).replace(/data:text\/javascript;base64,[A-Za-z0-9+/=]+/g, "<bundle>"));
  console.log(JSON.stringify(result.result.value, null, 2));
  if (!result.result.value?.passed) process.exitCode = 1;
} finally {
  socket?.close(); chrome.kill(); server.close(); await delay(500); await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}
