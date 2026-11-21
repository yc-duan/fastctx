// Measures how the conversation page scrolls: a conversation of so many
// turns made from the specimens' events and read as a shell reads a
// timeline (`?timeline=`), scrolled in a headless Chromium as a wheel
// scrolls it and as a dragged scrollbar or a middle-button scroll does
// (the position moved with no wheel event), each frame timed; then
// scrolled far up at once, to see whether what it passed is read in.
// The dev server must be running (`npm run dev`).
//
//   node scripts/scroll-perf.mjs [--url http://localhost:5317] [--turns 40,2000] [--delay 30] [--frames 240] [--speed 60] [--query x=y]

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
const turnCounts = option("turns", "40,2000").split(",").map(Number);
const delay = Number(option("delay", "30"));
const frames = Number(option("frames", "240"));
const speed = Number(option("speed", "60"));
const extra = option("query", "conversations");
const shots = option("shots", null);

const browsers = [
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
];
const browser = browsers.find((path) => existsSync(path));
if (!browser) throw new Error("no Chromium found");

const here = dirname(fileURLToPath(import.meta.url));
const kept = join(here, "..", "public", "_perf");
mkdirSync(kept, { recursive: true });
const profile = mkdtempSync(join(tmpdir(), "scroll-perf-"));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn(browser, [
  "--headless=new",
  `--remote-debugging-port=${port}`,
  `--user-data-dir=${profile}`,
  "--no-first-run",
  "--no-default-browser-check",
  "--force-color-profile=srgb",
  "--window-size=1280,860",
  "about:blank",
]);

async function json(path) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}${path}`);
      if (response.ok) return await response.json();
    } catch {
      // The browser is still starting.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("the browser did not answer");
}
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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
    } else if (data.method) {
      for (const listener of listeners) listener(data);
    }
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      serial += 1;
      pending.set(serial, { resolve, reject });
      socket.send(JSON.stringify({ id: serial, method, params }));
    });
  const once = (method) =>
    new Promise((resolve) => {
      const listener = (data) => {
        if (data.method === method) {
          listeners.delete(listener);
          resolve(data.params);
        }
      };
      listeners.add(listener);
    });
  const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result.value;
  const open = async (url) => {
    const loaded = once("Page.loadEventFired");
    await send("Page.navigate", { url });
    await loaded;
  };
  const problems = [];
  listeners.add((data) => {
    if (data.method === "Runtime.exceptionThrown") problems.push(data.params.exceptionDetails.exception?.description ?? "exception");
  });

  await send("Page.enable");
  await send("Runtime.enable");
  await send("Performance.enable");
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false });

  // The specimens' events, to make a long conversation of.
  await open(`${base}/?s=claude`);
  const samples = JSON.parse(
    await evaluate(`import("/src/specimen/sessions.ts").then((m) => JSON.stringify(["claude", "codex", "deeds", "steps", "trips"].map((name) => m.specimens[name]().events)))`),
  );

  const ids = new Set(["turn", "id", "call", "parent", "item_id", "tool_use_id"]);
  function named(value, found) {
    if (Array.isArray(value)) for (const one of value) named(one, found);
    else if (value && typeof value === "object") {
      for (const [key, inner] of Object.entries(value)) {
        if (ids.has(key) && typeof inner === "string") found.add(inner);
        else named(inner, found);
      }
    }
  }
  function renamed(value, found, suffix, at) {
    if (typeof value === "string") return found.has(value) ? `${value}~${suffix}` : value;
    if (Array.isArray(value)) return value.map((one) => renamed(one, found, suffix, at));
    if (value && typeof value === "object") {
      const next = {};
      for (const [key, inner] of Object.entries(value)) next[key] = key === "at" && typeof inner === "string" ? at : renamed(inner, found, suffix, at);
      return next;
    }
    return value;
  }
  function conversation(turns) {
    const lines = [];
    const began = Date.now() - turns * 60_000;
    let count = 0;
    for (let turn = 0; turn < turns; turn += 1) {
      const sample = samples[turn % samples.length];
      const found = new Set();
      named(sample, found);
      sample.forEach((event, index) => {
        const at = new Date(began + turn * 60_000 + index * 500).toISOString();
        count += 1;
        const key = `e${String(count).padStart(9, "0")}`;
        // An order key as a timeline gives them: its place among the
        // events, with the room left between two (`replica.ts`, ordinal).
        const order = (BigInt(count) << 20n).toString(16).padStart(16, "0");
        lines.push(JSON.stringify({ key, place: { order, item: key }, event: renamed(event, found, turn, at) }));
      });
    }
    return lines;
  }

  const summary = (times) => {
    const sorted = [...times].sort((a, b) => a - b);
    const pick = (share) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * share))];
    return {
      frames: times.length,
      median: +pick(0.5).toFixed(1),
      p95: +pick(0.95).toFixed(1),
      worst: +sorted[sorted.length - 1].toFixed(1),
      over25: times.filter((ms) => ms > 25).length,
      over50: times.filter((ms) => ms > 50).length,
    };
  };
  const metrics = async () => Object.fromEntries((await send("Performance.getMetrics")).metrics.map((one) => [one.name, one.value]));
  const spent = (before, after) => ({
    script: +((after.ScriptDuration - before.ScriptDuration) * 1000).toFixed(0),
    layout: +((after.LayoutDuration - before.LayoutDuration) * 1000).toFixed(0),
    style: +((after.RecalcStyleDuration - before.RecalcStyleDuration) * 1000).toFixed(0),
    task: +((after.TaskDuration - before.TaskDuration) * 1000).toFixed(0),
  });
  // Frames timed in the page while something scrolls it.
  const watch = `(() => { const times = []; let last = performance.now(); let on = true; const tick = () => { const now = performance.now(); times.push(now - last); last = now; if (on) requestAnimationFrame(tick); }; requestAnimationFrame(tick); window.__frames = { stop() { on = false; return times.slice(1); } }; })()`;
  const where = `(() => { const s = document.querySelector(".talk-scroller"); return s ? { top: Math.round(s.scrollTop), height: s.scrollHeight, view: s.clientHeight, nodes: document.querySelectorAll("*").length, gaps: document.querySelectorAll(".gap-span").length, loading: document.querySelectorAll(".gap-span.loading").length } : null; })()`;

  const results = [];
  for (const turns of turnCounts) {
    const lines = conversation(turns);
    const file = `t${turns}.jsonl`;
    writeFileSync(join(kept, file), lines.join("\n"));
    await open(`${base}/?s=claude&timeline=/_perf/${file}&partDelay=${delay}${extra ? `&${extra}` : ""}`);
    for (let waited = 0; waited < 100 && !(await evaluate(where)); waited += 1) await sleep(100);
    await sleep(1500);
    if (!(await evaluate(where))) {
      console.log(`no conversation shown: ${await evaluate(`location.href + " | " + document.body.innerText.slice(0, 400).replace(/\\s+/g, " ")`)}`);
      console.log(`problems: ${JSON.stringify(problems.slice(0, 5))}`);
      continue;
    }
    const row = { turns, events: lines.length, opened: await evaluate(where) };
    // What the page itself does to the view while it scrolls: each time it
    // sets the position (a correction the reader may see as a twitch), and
    // how many elements it puts on the page.
    await evaluate(`(() => {
      const s = document.querySelector(".talk-scroller");
      const own = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop");
      const made = { sets: 0, moving: 0, moved: 0, most: 0, added: 0, jumps: 0, worst: 0 };
      Object.defineProperty(s, "scrollTop", {
        configurable: true,
        get() { return own.get.call(this); },
        set(value) {
          const from = own.get.call(this);
          own.set.call(this, value);
          if (window.__driving) return;
          made.sets += 1;
          const by = Math.abs(own.get.call(this) - from);
          if (by >= 1) { made.moving += 1; made.moved += by; made.most = Math.max(made.most, by); }
          if (by >= 1) (window.__stacks ??= []).push(Math.round(own.get.call(this) - from) + (window.__scrolling ? " scrolling " : " resting ") + new Error().stack.split("\\n").slice(2, 7).map((line) => line.trim().replace(/\\(?http.*\\/src\\//, "(").replace(/\\?t=\\d+/, "")).join(" < "));
        },
      });
      new MutationObserver((records) => { for (const record of records) made.added += record.addedNodes.length; }).observe(s, { childList: true, subtree: true });
      // What the reader sees move other than by the scroll: a block in view
      // that is not where the last frame's scroll left it.
      let last = null;
      const look = () => {
        const top = own.get.call(s);
        const box = s.getBoundingClientRect();
        let seen = null;
        for (const block of s.querySelector(".conversation").children) {
          // A space, or the stand-in for turns not read, shows nothing to move.
          if (block.classList.contains("transcript-space") || block.querySelector(":scope > .gap-span")) continue;
          const at = block.getBoundingClientRect();
          if (at.bottom > box.top + 80 && at.height > 0) { seen = { block, at: at.top }; break; }
        }
        // Moved on the screen by more than a frame's scrolling could move it.
        if (last && seen && last.block === seen.block) {
          const off = Math.abs(seen.at - last.at);
          if (off > ${speed} * 4 + 4) { made.jumps += 1; made.worst = Math.max(made.worst, off); }
        }
        last = seen ? { ...seen, top } : null;
        requestAnimationFrame(look);
      };
      requestAnimationFrame(look);
      window.__taken = () => { const now = { ...made, moved: Math.round(made.moved), most: Math.round(made.most), worst: Math.round(made.worst) }; for (const key of Object.keys(made)) made[key] = 0; return now; };
    })()`);

    // As a dragged scrollbar or a middle-button scroll moves it: the
    // position set each frame, no wheel event.
    let before = await metrics();
    const moved = await evaluate(`(async () => { window.__scrolling = true; const s = document.querySelector(".talk-scroller"); const times = []; let last = performance.now(); for (let i = 0; i < ${frames}; i += 1) { window.__driving = true; s.scrollTop -= ${speed}; window.__driving = false; await new Promise((r) => requestAnimationFrame(r)); const now = performance.now(); times.push(now - last); last = now; } window.__scrolling = false; return times; })()`);
    row.position = { ...summary(moved), ...spent(before, await metrics()), made: await evaluate(`window.__taken()`), at: await evaluate(where) };
    await sleep(1000);

    // As a wheel scrolls it.
    await evaluate(watch);
    await evaluate(`window.__scrolling = true`);
    before = await metrics();
    for (let i = 0; i < frames; i += 1) {
      await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: 640, y: 400, deltaX: 0, deltaY: -speed });
      await sleep(12);
    }
    await sleep(100);
    await evaluate(`window.__scrolling = false`);
    row.wheel = { ...summary(await evaluate(`window.__frames.stop()`)), ...spent(before, await metrics()), made: await evaluate(`window.__taken()`), at: await evaluate(where) };
    await sleep(1000);

    // Far up at once, then left alone: what it passed and where it stopped
    // is read in, or stays a gap being read.
    before = await metrics();
    const far = await evaluate(`(async () => { const s = document.querySelector(".talk-scroller"); const times = []; let last = performance.now(); for (let i = 0; i < 90; i += 1) { window.__driving = true; s.scrollTop -= 2500; window.__driving = false; await new Promise((r) => requestAnimationFrame(r)); const now = performance.now(); times.push(now - last); last = now; } return times; })()`);
    const settling = [];
    for (let waited = 0; waited < 40; waited += 1) {
      await sleep(250);
      const now = await evaluate(where);
      settling.push(now.loading + now.gaps > 0 ? `${now.gaps}/${now.loading}` : "-");
      if (waited > 8 && settling.slice(-8).every((one) => one === "-")) break;
    }
    row.far = { ...summary(far), ...spent(before, await metrics()), at: await evaluate(where), gapsAndLoadingEachQuarterSecond: settling.join(" ") };
    if (shots) {
      mkdirSync(shots, { recursive: true });
      const shot = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(shots, `far-${turns}.png`), Buffer.from(shot.data, "base64"));
    }
    results.push(row);
    console.log(JSON.stringify(row));
    if (option("stacks", null)) for (const stack of (await evaluate(`JSON.stringify(window.__stacks ?? [])`).then(JSON.parse))) console.log(`  set ${stack}`);
  }
  if (problems.length) console.log(`problems: ${JSON.stringify([...new Set(problems)].slice(0, 8))}`);
  socket.close();
} finally {
  chrome.kill();
  await sleep(300);
  try {
    rmSync(profile, { recursive: true, force: true });
    rmSync(kept, { recursive: true, force: true });
  } catch {
    // Left for the next run to clear.
  }
}
