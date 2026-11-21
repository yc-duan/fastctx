// Measures what the page shows as the side opens and shuts beside a
// conversation (`carry.ts`): where the column, a block of it in view, the
// composer and the side's sheet stand at every frame, as painted, in a
// headless Chromium. A part that is not where the frame before left it by
// more than a glide moves it in a frame has leapt. The dev server must be
// running (`npm run dev`).
//
//   node scripts/side-carry.mjs [--part side|sidebar] [--url http://localhost:5317] [--turns 40] [--sizes 1600x900,1280x860,1100x860] [--up 0,1500] [--frames] [--trace]

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
const sizes = option("sizes", "1600x900,1280x860,1100x860").split(",").map((size) => size.split("x").map(Number));
const ups = option("up", "0,1500").split(",").map(Number);
const extra = option("query", "conversations");
const all = args.includes("--frames");
const trace = args.includes("--trace");
// What is turned: the side beside the conversations, or the sidebar across
// the desk (Ctrl+B), which carries the page the same way.
const turn = option("part", "side") === "sidebar"
  ? `dispatchEvent(new KeyboardEvent("keydown", { key: "b", ctrlKey: true, bubbles: true }))`
  : `document.querySelector(".sp-toggle").click()`;

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
  "--window-size=1600,900",
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

  // Where the parts stand after each frame is painted: asked for from the
  // frame's own callback, answered once the frame is done.
  const record = `(() => {
    const s = document.querySelector(".talk-scroller");
    const column = s.querySelector(".transcript");
    const box = s.getBoundingClientRect();
    const blocks = [...column.children].filter((block) => !block.classList.contains("transcript-space") && block.getBoundingClientRect().height > 0);
    const seen = blocks.filter((block) => { const at = block.getBoundingClientRect(); return at.bottom > box.top + 40 && at.top < box.bottom - 40; });
    const first = seen[0], last = seen[seen.length - 1];
    const composer = [...document.querySelectorAll(".composer-dock > *")].find((part) => part.getBoundingClientRect().height > 0);
    const words = composer?.querySelector("[contenteditable], textarea") ?? null;
    const head = document.querySelector(".page-head-title");
    const sheet = document.querySelector(".sp-sheet");
    const host = document.querySelector(".sp-host");
    const frames = [];
    let on = true;
    const channel = new MessageChannel();
    const at = (element) => { if (!element || !element.isConnected) return null; const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height].map((n) => Math.round(n * 10) / 10); };
    channel.port1.onmessage = () => {
      frames.push({ t: Math.round(performance.now()), shown: host.classList.contains("sp-shown") !== !!document.querySelector(".folded"), column: at(column), first: at(first), last: at(last), composer: at(composer), words: at(words), head: at(head), sheet: at(sheet), scroller: at(s), top: Math.round(s.scrollTop * 10) / 10, tall: s.scrollHeight });
      if (on) requestAnimationFrame(() => channel.port2.postMessage(0));
    };
    requestAnimationFrame(() => channel.port2.postMessage(0));
    window.__carried = { stop() { on = false; return frames; } };
  })()`;
  const where = `(() => { const s = document.querySelector(".talk-scroller"); return s ? { top: Math.round(s.scrollTop), height: s.scrollHeight, view: s.clientHeight, toggle: !!document.querySelector(".sp-toggle"), shown: !!document.querySelector(".sp-host.sp-shown"), over: !!document.querySelector(".sp-host.sp-over") } : null; })()`;
  const parts = ["column", "first", "last", "composer", "words", "head", "sheet"];
  const line = (frame, from) =>
    `${String(frame.t - from).padStart(5)}ms ${frame.shown ? "a" : "b"} ` +
    parts.map((part) => `${part} ${frame[part] ? `${frame[part][0]},${frame[part][1]} ${frame[part][2]}x${frame[part][3]}` : "-"}`).join(" | ") +
    ` | top ${frame.top} of ${frame.tall}`;

  const lines = conversation(turns);
  const file = `c${turns}.jsonl`;
  writeFileSync(join(kept, file), lines.join("\n"));
  for (const [width, height] of sizes) {
    await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    for (const up of ups) {
      await open(`${base}/?s=claude&timeline=/_perf/${file}&partDelay=30${extra ? `&${extra}` : ""}`);
      for (let waited = 0; waited < 100 && !(await evaluate(where)); waited += 1) await sleep(100);
      await sleep(1500);
      if (up) {
        await evaluate(`(() => { const s = document.querySelector(".talk-scroller"); s.scrollTop -= ${up}; })()`);
        await sleep(1200);
      }
      console.log(`== ${width}x${height}, ${up ? `${up}px up from the end` : "at the end"}: ${JSON.stringify(await evaluate(where))}`);
      if (trace) {
        // Each time the page sets where the view is scrolled to: from what,
        // to what, how tall the page was then, and who set it.
        await evaluate(`(() => {
          const s = document.querySelector(".talk-scroller");
          const own = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop");
          window.__sets = [];
          Object.defineProperty(s, "scrollTop", {
            configurable: true,
            get() { return own.get.call(this); },
            set(value) {
              const from = own.get.call(this);
              const tall = this.scrollHeight;
              own.set.call(this, value);
              window.__sets.push(Math.round(performance.now()) + "ms " + from + " -> " + own.get.call(this) + " (asked " + Math.round(value) + ", page " + tall + ") " + new Error().stack.split("\\n").slice(2, 6).map((line) => line.trim().replace(/\\(?http.*\\/src\\//, "(").replace(/\\?t=\\d+/, "")).join(" < "));
            },
          });
        })()`);
      }
      for (const act of ["turned", "turned back"]) {
        await evaluate(record);
        await sleep(150);
        // Turned from a frame's own callback: turned between two frames, a
        // sample taken before the next would lay the page out before the
        // carrying had begun, and tell of a leap never painted.
        await evaluate(`requestAnimationFrame(() => { ${turn}; })`);
        await sleep(900);
        const frames = await evaluate(`window.__carried.stop()`);
        const turned = frames.findIndex((frame, index) => index > 0 && frame.shown !== frames[0].shown);
        if (turned < 1) {
          console.log(`  ${act}: nothing turned (${frames.length} frames)`);
          continue;
        }
        const before = frames[turned - 1];
        const from = before.t;
        // How far each part went between two frames, the largest step of
        // the glide after its first, and the step at its first frame.
        const step = (a, b, part, axis) => (a[part] && b[part] ? b[part][axis] - a[part][axis] : 0);
        const said = [];
        for (const part of parts) {
          const firstStep = [step(before, frames[turned], part, 0), step(before, frames[turned], part, 1)];
          const sized = [step(before, frames[turned], part, 2), step(before, frames[turned], part, 3)];
          let most = [0, 0];
          for (let index = turned + 1; index < frames.length; index += 1) {
            for (const axis of [0, 1]) most[axis] = Math.max(most[axis], Math.abs(step(frames[index - 1], frames[index], part, axis)));
          }
          const whole = [step(before, frames[frames.length - 1], part, 0), step(before, frames[frames.length - 1], part, 1)];
          said.push(`${part}: first frame ${firstStep.map((n) => n.toFixed(1)).join(",")} (size ${sized.map((n) => n.toFixed(1)).join(",")}), largest later step ${most.map((n) => n.toFixed(1)).join(",")}, in all ${whole.map((n) => n.toFixed(1)).join(",")}`);
        }
        console.log(`  ${act}: ${frames.length} frames, turned at frame ${turned}; scroll ${before.top} -> ${frames[turned].top} -> ${frames[frames.length - 1].top}, height ${before.tall} -> ${frames[frames.length - 1].tall}`);
        for (const one of said) console.log(`    ${one}`);
        if (all) {
          const shownFrames = frames.slice(Math.max(0, turned - 2), turned + 8).concat(frames.slice(turned + 8).filter((_, index) => index % 4 === 0), frames.slice(-3));
          for (const frame of shownFrames) console.log(`    ${line(frame, from)}`);
        }
        if (trace) for (const set of await evaluate(`JSON.stringify(window.__sets.splice(0))`).then(JSON.parse)) console.log(`    set ${set}`);
        await sleep(400);
      }
    }
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
