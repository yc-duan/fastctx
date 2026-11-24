// Where the main thread's time goes while a conversation's side or the
// sidebar opens and shuts, in a headless Chromium: a trace of the glide,
// summed by what was done, and each setting of styles and each laying out by
// what asked for it. `side-room.mjs` says where things stand at each frame;
// this says what each frame cost and why. A larger window draws more of the
// conversation: what grows with it grows with a real conversation. The dev
// server must be running (`npm run dev`).
//
//   node scripts/glide-trace.mjs [--url http://localhost:5317] [--turns 40] [--size 1600x900] [--slow 1] [--part side|sidebar] [--try <js>]

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const at = args.indexOf(`--${name}`);
  return at === -1 ? fallback : args[at + 1];
};
const base = option("url", "http://localhost:5317");
const turns = Number(option("turns", "40"));
const [width, height] = option("size", "1600x900").split("x").map(Number);
const slow = Number(option("slow", "1"));
const part = option("part", "side");
const tried = option("try", "");
const extra = option("query", "conversations");
const turn = part === "sidebar"
  ? `dispatchEvent(new KeyboardEvent("keydown", { key: "b", ctrlKey: true, bubbles: true }))`
  : `document.querySelector(".sp-toggle").click()`;

const browser = ["C:/Program Files/Google/Chrome/Application/chrome.exe", "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"].find((path) => existsSync(path));
const kept = join(dirname(fileURLToPath(import.meta.url)), "..", "public", "_perf");
mkdirSync(kept, { recursive: true });
const profile = mkdtempSync(join(tmpdir(), "glide-trace-"));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn(browser, ["--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--force-color-profile=srgb", `--window-size=${width},${height}`, "about:blank"]);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function json(path) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}${path}`);
      if (response.ok) return await response.json();
    } catch {}
    await sleep(100);
  }
  throw new Error("the browser did not answer");
}

try {
  const [target] = (await json("/json/list")).filter((entry) => entry.type === "page");
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve) => socket.addEventListener("open", resolve, { once: true }));
  let serial = 0;
  const pending = new Map();
  const listeners = new Set();
  socket.addEventListener("message", (message) => {
    const data = JSON.parse(message.data);
    if (data.id && pending.has(data.id)) {
      const { resolve, reject } = pending.get(data.id);
      pending.delete(data.id);
      if (data.error) reject(new Error(data.error.message));
      else if (data.result?.exceptionDetails) reject(new Error(data.result.exceptionDetails.exception?.description ?? data.result.exceptionDetails.text));
      else resolve(data.result);
    } else if (data.method) for (const listener of listeners) listener(data);
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => { serial += 1; pending.set(serial, { resolve, reject }); socket.send(JSON.stringify({ id: serial, method, params })); });
  const once = (method) => new Promise((resolve) => { const listener = (data) => { if (data.method === method) { listeners.delete(listener); resolve(data.params); } }; listeners.add(listener); });
  const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result.value;
  const open = async (url) => { const loaded = once("Page.loadEventFired"); await send("Page.navigate", { url }); await loaded; };

  await send("Page.enable");
  await send("Runtime.enable");
  await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  await open(`${base}/?s=claude`);
  const samples = JSON.parse(await evaluate(`import("/src/specimen/sessions.ts").then((m) => JSON.stringify(["claude", "codex", "deeds", "steps", "trips"].map((name) => m.specimens[name]().events)))`));
  const ids = new Set(["turn", "id", "call", "parent", "item_id", "tool_use_id"]);
  function named(value, found) {
    if (Array.isArray(value)) for (const one of value) named(one, found);
    else if (value && typeof value === "object") for (const [key, inner] of Object.entries(value)) { if (ids.has(key) && typeof inner === "string") found.add(inner); else named(inner, found); }
  }
  function renamed(value, found, suffix, at) {
    if (typeof value === "string") return found.has(value) ? `${value}~${suffix}` : value;
    if (Array.isArray(value)) return value.map((one) => renamed(one, found, suffix, at));
    if (value && typeof value === "object") { const next = {}; for (const [key, inner] of Object.entries(value)) next[key] = key === "at" && typeof inner === "string" ? at : renamed(inner, found, suffix, at); return next; }
    return value;
  }
  const lines = [];
  const began = Date.now() - turns * 60_000;
  let count = 0;
  for (let at = 0; at < turns; at += 1) {
    const sample = samples[at % samples.length];
    const found = new Set();
    named(sample, found);
    sample.forEach((event, index) => {
      const when = new Date(began + at * 60_000 + index * 500).toISOString();
      count += 1;
      const key = `e${String(count).padStart(9, "0")}`;
      const order = (BigInt(count) << 20n).toString(16).padStart(16, "0");
      lines.push(JSON.stringify({ key, place: { order, item: key }, event: renamed(event, found, at, when) }));
    });
  }
  const file = `t${turns}.jsonl`;
  writeFileSync(join(kept, file), lines.join("\n"));

  await evaluate(`localStorage.setItem("side-dock", "right")`);
  await open(`${base}/?s=claude&timeline=/_perf/${file}&partDelay=30${extra ? `&${extra}` : ""}`);
  const where = `(() => { const s = document.querySelector(".talk-scroller"); return s ? { blocks: s.querySelectorAll(".transcript > .block").length, nodes: document.querySelectorAll("*").length, page: document.querySelectorAll(".conversation-page *").length, shown: !!document.querySelector(".sp-host.sp-shown") } : null; })()`;
  for (let waited = 0; waited < 100 && !(await evaluate(where)); waited += 1) await sleep(100);
  await sleep(1500);
  if (tried) await evaluate(tried);
  console.log(`== ${width}x${height}${slow > 1 ? `, processor ${slow}x slower` : ""}: ${JSON.stringify(await evaluate(where))}`);
  await send("Emulation.setCPUThrottlingRate", { rate: slow });

  for (const act of ["turned", "turned back"]) {
    // Frames as the page makes them, with no reading of where things stand.
    await evaluate(`(() => { const frames = []; let on = true; const tick = (now) => { frames.push(now); if (on) requestAnimationFrame(tick); }; requestAnimationFrame(tick); window.__frames = { stop() { on = false; return frames; } }; })()`);
    await sleep(200);
    const traced = [];
    const tracing = (data) => { if (data.method === "Tracing.dataCollected") traced.push(...data.params.value); };
    listeners.add(tracing);
    await send("Tracing.start", { transferMode: "ReportEvents", traceConfig: { includedCategories: ["devtools.timeline", "disabled-by-default-devtools.timeline", "disabled-by-default-devtools.timeline.stack", "blink", "blink.user_timing", "cc", "v8"] } });
    await evaluate(`performance.mark("glide-asked"); requestAnimationFrame(() => { ${turn}; })`);
    await sleep(900 + 300 * slow);
    const done = once("Tracing.tracingComplete");
    await send("Tracing.end");
    await done;
    listeners.delete(tracing);
    const frames = await evaluate(`window.__frames.stop()`);

    const mark = traced.find((event) => event.name === "glide-asked");
    const from = mark ? mark.ts : Math.min(...traced.filter((event) => event.ts).map((event) => event.ts));
    const until = from + (620 + 80 * slow) * 1000;
    const main = mark?.tid;
    const within = traced.filter((event) => event.ph === "X" && event.tid === main && event.ts >= from && event.ts < until);
    const sums = new Map();
    for (const event of within) {
      const one = sums.get(event.name) ?? { count: 0, total: 0, most: 0 };
      one.count += 1;
      one.total += event.dur ?? 0;
      one.most = Math.max(one.most, event.dur ?? 0);
      sums.set(event.name, one);
    }
    console.log(`  ${act}: on the main thread over ${((until - from) / 1000).toFixed(0)} ms, by what was done (count, total ms, longest ms):`);
    for (const [name, one] of [...sums].sort((a, b) => b[1].total - a[1].total).slice(0, 26)) console.log(`    ${name.padEnd(46)} ${String(one.count).padStart(5)} ${(one.total / 1000).toFixed(1).padStart(8)} ${(one.most / 1000).toFixed(1).padStart(7)}`);
    // Each setting of styles: who asked, how many elements.
    const asked = new Map();
    for (const event of within.filter((event) => event.name === "UpdateLayoutTree")) {
      const stack = event.args?.beginData?.stackTrace ?? [];
      const frame = stack.find((one) => one.url && !one.url.includes("node_modules"));
      const who = frame ? `${frame.url.split("/").slice(-2).join("/").replace(/\?.*$/, "")}:${frame.lineNumber} ${frame.functionName}` : "the page's own update";
      const one = asked.get(who) ?? { count: 0, total: 0, elements: 0, most: 0 };
      one.count += 1;
      one.total += event.dur ?? 0;
      one.elements += event.args?.elementCount ?? 0;
      one.most = Math.max(one.most, event.args?.elementCount ?? 0);
      asked.set(who, one);
    }
    console.log(`    styles, by who asked (count, total ms, elements on average, at most):`);
    for (const [who, one] of [...asked].sort((a, b) => b[1].total - a[1].total).slice(0, 14)) console.log(`      ${who.slice(0, 70).padEnd(70)} ${String(one.count).padStart(4)} ${(one.total / 1000).toFixed(1).padStart(7)} ${(one.elements / one.count).toFixed(0).padStart(6)} ${String(one.most).padStart(6)}`);
    const layouts = new Map();
    for (const event of within.filter((event) => event.name === "Layout")) {
      const stack = event.args?.beginData?.stackTrace ?? [];
      const frame = stack.find((one) => one.url && !one.url.includes("node_modules"));
      const who = frame ? `${frame.url.split("/").slice(-2).join("/").replace(/\?.*$/, "")}:${frame.lineNumber} ${frame.functionName}` : "the page's own update";
      const one = layouts.get(who) ?? { count: 0, total: 0 };
      one.count += 1;
      one.total += event.dur ?? 0;
      layouts.set(who, one);
    }
    console.log(`    laying out, by who asked (count, total ms):`);
    for (const [who, one] of [...layouts].sort((a, b) => b[1].total - a[1].total).slice(0, 8)) console.log(`      ${who.slice(0, 70).padEnd(70)} ${String(one.count).padStart(4)} ${(one.total / 1000).toFixed(1).padStart(7)}`);
    const spans = frames.slice(1).map((now, index) => now - frames[index]).filter((span) => span > 0);
    const glide = spans.slice(Math.max(0, Math.floor(200 / 16.7) - 2), Math.max(0, Math.floor(200 / 16.7) - 2) + Math.ceil(560 / 16.7));
    const sorted = [...glide].sort((a, b) => a - b);
    console.log(`    frames around the glide: ${glide.length}; ${sorted[Math.floor(sorted.length / 2)]?.toFixed(1)} ms in the middle, ${sorted[sorted.length - 1]?.toFixed(1)} ms at most, ${glide.filter((span) => span > 25).length} over 25 ms`);
    await sleep(400);
  }
  socket.close();
} finally {
  chrome.kill();
  await sleep(300);
  try { rmSync(profile, { recursive: true, force: true }); rmSync(kept, { recursive: true, force: true }); } catch {}
}
