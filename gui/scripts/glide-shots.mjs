// Pictures of a conversation's side or the sidebar opening and shutting, in
// a headless Chromium: the glide made ten times as long, and the page taken
// shut, at three moments of its opening, open, at three of its shutting and
// shut again. The glide is lengthened by its own length (`--glide`), which
// what ends it is timed by as well; slowing the animations alone leaves it
// ended at its usual time. The dev server must be running (`npm run dev`).
//
//   node scripts/glide-shots.mjs --out <dir> [--url http://localhost:5317] [--size 1600x900] [--turns 40] [--up 0] [--part side|sidebar] [--name glide] [--rate 0.1]
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const option = (name, fallback) => { const at = args.indexOf(`--${name}`); return at === -1 ? fallback : args[at + 1]; };
const base = option("url", "http://localhost:5317");
const out = option("out", ".");
const name = option("name", "glide");
const turns = Number(option("turns", "40"));
const up = Number(option("up", "0"));
const [width, height] = option("size", "1600x900").split("x").map(Number);
const part = option("part", "side");
const rate = Number(option("rate", "0.1"));
const turn = part === "sidebar" ? `dispatchEvent(new KeyboardEvent("keydown", { key: "b", ctrlKey: true, bubbles: true }))` : `document.querySelector(".sp-toggle").click()`;
const browser = ["C:/Program Files/Google/Chrome/Application/chrome.exe", "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"].find((path) => existsSync(path));
const kept = join(dirname(fileURLToPath(import.meta.url)), "..", "public", "_shots");
mkdirSync(kept, { recursive: true });
mkdirSync(out, { recursive: true });
const profile = mkdtempSync(join(tmpdir(), "shots-"));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn(browser, ["--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--force-color-profile=srgb", `--window-size=${width},${height}`, "about:blank"]);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function json(path) { for (let n = 0; n < 100; n += 1) { try { const r = await fetch(`http://127.0.0.1:${port}${path}`); if (r.ok) return await r.json(); } catch {} await sleep(100); } throw new Error("the browser did not answer"); }
try {
  const [target] = (await json("/json/list")).filter((entry) => entry.type === "page");
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve) => socket.addEventListener("open", resolve, { once: true }));
  let serial = 0; const pending = new Map(); const listeners = new Set();
  socket.addEventListener("message", (message) => { const data = JSON.parse(message.data); if (data.id && pending.has(data.id)) { const { resolve, reject } = pending.get(data.id); pending.delete(data.id); if (data.error) reject(new Error(data.error.message)); else resolve(data.result); } else if (data.method) for (const l of listeners) l(data); });
  const send = (method, params = {}) => new Promise((resolve, reject) => { serial += 1; pending.set(serial, { resolve, reject }); socket.send(JSON.stringify({ id: serial, method, params })); });
  const once = (method) => new Promise((resolve) => { const l = (data) => { if (data.method === method) { listeners.delete(l); resolve(data.params); } }; listeners.add(l); });
  const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result.value;
  const open = async (url) => { const loaded = once("Page.loadEventFired"); await send("Page.navigate", { url }); await loaded; };
  await send("Page.enable"); await send("Runtime.enable");
  await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  await open(`${base}/?s=claude`);
  const samples = JSON.parse(await evaluate(`import("/src/specimen/sessions.ts").then((m) => JSON.stringify(["claude", "codex", "deeds", "steps", "trips"].map((name) => m.specimens[name]().events)))`));
  const ids = new Set(["turn", "id", "call", "parent", "item_id", "tool_use_id"]);
  const named = (value, found) => { if (Array.isArray(value)) for (const one of value) named(one, found); else if (value && typeof value === "object") for (const [key, inner] of Object.entries(value)) { if (ids.has(key) && typeof inner === "string") found.add(inner); else named(inner, found); } };
  const renamed = (value, found, suffix, at) => { if (typeof value === "string") return found.has(value) ? `${value}~${suffix}` : value; if (Array.isArray(value)) return value.map((one) => renamed(one, found, suffix, at)); if (value && typeof value === "object") { const next = {}; for (const [key, inner] of Object.entries(value)) next[key] = key === "at" && typeof inner === "string" ? at : renamed(inner, found, suffix, at); return next; } return value; };
  const lines = []; const began = Date.now() - turns * 60_000; let count = 0;
  for (let at = 0; at < turns; at += 1) { const sample = samples[at % samples.length]; const found = new Set(); named(sample, found); sample.forEach((event, index) => { const when = new Date(began + at * 60_000 + index * 500).toISOString(); count += 1; const key = `e${String(count).padStart(9, "0")}`; const order = (BigInt(count) << 20n).toString(16).padStart(16, "0"); lines.push(JSON.stringify({ key, place: { order, item: key }, event: renamed(event, found, at, when) })); }); }
  writeFileSync(join(kept, "c.jsonl"), lines.join("\n"));
  await evaluate(`localStorage.setItem("side-dock", "right")`);
  await open(`${base}/?s=claude&timeline=/_shots/c.jsonl&partDelay=30&conversations`);
  for (let waited = 0; waited < 100 && !(await evaluate(`!!document.querySelector(".talk-scroller .transcript > .block")`)); waited += 1) await sleep(100);
  await sleep(1500);
  if (up) { await evaluate(`(() => { const s = document.querySelector(".talk-scroller"); s.scrollTop -= ${up}; })()`); await sleep(1200); }
  const shot = async (label) => { const { data } = await send("Page.captureScreenshot", { format: "png" }); writeFileSync(join(out, `${name}-${label}.png`), Buffer.from(data, "base64")); };
  await shot("0-shut");
  const glide = 520 / rate;
  await evaluate(`document.documentElement.style.setProperty("--glide", "${glide}ms")`);
  await evaluate(`requestAnimationFrame(() => { ${turn}; })`);
  for (const share of [0.15, 0.4, 0.7]) { await sleep(glide * (share - (share === 0.15 ? 0 : share === 0.4 ? 0.15 : 0.4))); await shot(`1-open-${Math.round(share * 100)}`); }
  await sleep(glide * 0.6);
  await sleep(900);
  await shot("2-open");
  await evaluate(`requestAnimationFrame(() => { ${turn}; })`);
  for (const share of [0.15, 0.4, 0.7]) { await sleep(glide * (share - (share === 0.15 ? 0 : share === 0.4 ? 0.15 : 0.4))); await shot(`3-shut-${Math.round(share * 100)}`); }
  await sleep(glide * 0.6);
  await sleep(900);
  await shot("4-shut");
  console.log("done");
  socket.close();
} finally {
  chrome.kill(); await sleep(300);
  try { rmSync(profile, { recursive: true, force: true }); rmSync(kept, { recursive: true, force: true }); } catch {}
}
