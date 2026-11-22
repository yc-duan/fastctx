// Measures the side and the sidebar opening and shutting beside a
// conversation (`app/room.ts`), in a headless Chromium: at every frame,
// where the sheet stands, how wide the conversation's column and its words
// are, and how long the frame took; and what the glide cost the page in
// laying out, in styles and in script. The words are to narrow a little at
// every frame, never all at once, the column's edge is to stay at the
// sheet's, and no frame is to be long. The dev server must be running
// (`npm run dev`).
//
//   node scripts/side-room.mjs [--part side|sidebar] [--url http://localhost:5317] [--turns 40] [--sizes 1600x900,1280x860,1100x860] [--up 0,1500] [--slow 4] [--frames]

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
const slow = Number(option("slow", "1"));
// What sets styles again as it glides: how many elements each time, and why.
const styles = args.includes("--styles");
// Where the side stands, and whether it is turned again before its glide
// is over, so many milliseconds in.
const dock = option("dock", "right");
const again = Number(option("again", "0"));
// Script run in the page before it is measured, to try something out.
const tried = option("try", "");
// What is turned: the side beside the conversations, or the sidebar across
// the desk (Ctrl+B), which carries the page the same way.
const part = option("part", "side");
const turn = part === "none" ? "0" : part === "sidebar"
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

  // Where the parts stand at each frame, read in the frame's own callback
  // before anything is written: what the frame before laid out and painted.
  const record = `(() => {
    const s = document.querySelector(".talk-scroller");
    const column = s.querySelector(".transcript");
    const box = s.getBoundingClientRect();
    const blocks = [...column.children].filter((block) => !block.classList.contains("transcript-space") && block.getBoundingClientRect().height > 0);
    const seen = blocks.filter((block) => { const at = block.getBoundingClientRect(); return at.bottom > box.top + 40 && at.top < box.bottom - 40; });
    const prose = seen.map((block) => block.querySelector(".prose, .say")).find(Boolean) ?? seen[0];
    const composer = [...document.querySelectorAll(".composer-dock > *")].find((part) => part.getBoundingClientRect().height > 0);
    const head = document.querySelector(".page-head-actions") ?? document.querySelector(".page-head-title");
    const sheet = document.querySelector(${JSON.stringify(part === "sidebar" ? ".sidebar-frame" : ".sp-sheet")});
    const panes = document.querySelector(".desk-panes");
    const frames = [];
    let on = true;
    const at = (element) => { if (!element || !element.isConnected) return null; const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height].map((n) => Math.round(n * 10) / 10); };
    // Asked for from the frame's own callback and answered once the frame
    // is done: what it laid out and painted, at the time it was made for.
    const channel = new MessageChannel();
    let made = 0;
    channel.port1.onmessage = () => {
      frames.push({ t: Math.round(made * 10) / 10, sheet: at(sheet), panes: at(panes), scroller: at(s), column: at(column), words: at(prose), composer: at(composer), head: at(head), top: Math.round(s.scrollTop * 10) / 10, tall: s.scrollHeight, foot: Math.round((s.scrollHeight - s.scrollTop - s.clientHeight) * 10) / 10 });
      if (on) requestAnimationFrame(tick);
    };
    const tick = (now) => { made = now; channel.port2.postMessage(0); };
    requestAnimationFrame(tick);
    window.__glided = { stop() { on = false; return frames; } };
  })()`;
  const where = `(() => { const s = document.querySelector(".talk-scroller"); return s ? { top: Math.round(s.scrollTop), height: s.scrollHeight, view: s.clientHeight, blocks: s.querySelectorAll(".transcript > .block").length, nodes: document.querySelectorAll("*").length, shown: !!document.querySelector(".sp-host.sp-shown"), folded: !!document.querySelector(".shell.folded"), over: !!document.querySelector(".sp-host.sp-over") } : null; })()`;
  const metrics = async () => Object.fromEntries((await send("Performance.getMetrics")).metrics.map((metric) => [metric.name, metric.value]));
  const ms = (after, before, name) => ((after[name] - before[name]) * 1000).toFixed(0);

  const lines = conversation(turns);
  const file = `c${turns}.jsonl`;
  writeFileSync(join(kept, file), lines.join("\n"));
  let bad = 0;
  for (const [width, height] of sizes) {
    await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    for (const up of ups) {
      await send("Emulation.setCPUThrottlingRate", { rate: 1 });
      await evaluate(`localStorage.setItem("side-dock", ${JSON.stringify(dock)})`);
      await open(`${base}/?s=claude&timeline=/_perf/${file}&partDelay=30${extra ? `&${extra}` : ""}`);
      for (let waited = 0; waited < 100 && !(await evaluate(where)); waited += 1) await sleep(100);
      await sleep(1500);
      if (up) {
        await evaluate(`(() => { const s = document.querySelector(".talk-scroller"); s.scrollTop -= ${up}; })()`);
        await sleep(1200);
      }
      if (tried) await evaluate(tried);
      console.log(`== ${width}x${height}, ${up ? `${up}px up from the end` : "at the end"}${slow > 1 ? `, processor ${slow}x slower` : ""}: ${JSON.stringify(await evaluate(where))}`);
      await send("Emulation.setCPUThrottlingRate", { rate: slow });
      for (const act of ["turned", "turned back"]) {
        await evaluate(record);
        await sleep(200);
        const traced = [];
        const tracing = (data) => { if (data.method === "Tracing.dataCollected") traced.push(...data.params.value); };
        if (styles) {
          listeners.add(tracing);
          await send("Tracing.start", { transferMode: "ReportEvents", traceConfig: { includedCategories: ["devtools.timeline", "disabled-by-default-devtools.timeline.invalidationTracking"] } });
        }
        const before = await metrics();
        await evaluate(`requestAnimationFrame(() => { ${turn}; })`);
        if (again) {
          await sleep(again);
          await evaluate(`requestAnimationFrame(() => { ${turn}; })`);
        }
        await sleep(900 + 300 * slow);
        const after = await metrics();
        if (styles) {
          const done = once("Tracing.tracingComplete");
          await send("Tracing.end");
          await done;
          listeners.delete(tracing);
          const recalcs = traced.filter((event) => event.name === "UpdateLayoutTree" && event.ph === "X");
          const counts = recalcs.map((event) => event.args?.elementCount ?? 0).sort((a, b) => b - a);
          console.log(`    styles set again ${recalcs.length} times, ${(recalcs.reduce((sum, event) => sum + event.dur, 0) / 1000).toFixed(0)} ms; elements each time, most first: ${counts.slice(0, 40).join(" ")}`);
          const began = Math.min(...recalcs.map((event) => event.ts));
          console.log(`    the large ones, when (ms) and how many elements: ${recalcs.filter((event) => (event.args?.elementCount ?? 0) > 400).map((event) => `${((event.ts - began) / 1000).toFixed(0)}:${event.args.elementCount}/${(event.dur / 1000).toFixed(1)}ms`).join(" ")}`);
          // What asked for each of the large ones: what was changed since
          // the styles were set before it, and by which script.
          const ordered = traced.filter((event) => event.ts).sort((x, y) => x.ts - y.ts);
          let since = [];
          for (const event of ordered) {
            if (event.name === "UpdateLayoutTree" && event.ph === "X") {
              if ((event.args?.elementCount ?? 0) > 900) {
                const said = new Map();
                for (const one of since) said.set(one, (said.get(one) ?? 0) + 1);
                console.log(`      at ${((event.ts - began) / 1000).toFixed(0)} ms, ${event.args.elementCount} elements: ${[...said].map(([one, count]) => `${count}x ${one}`).slice(0, 6).join(" ; ").slice(0, 700)}`);
              }
              since = [];
            } else if (event.name === "ScheduleStyleInvalidationTracking" || event.name === "StyleRecalcInvalidationTracking") {
              const data = event.args?.data ?? {};
              if (event.name === "StyleRecalcInvalidationTracking" && data.reason === "Animation") continue;
              const frame = (data.stackTrace ?? []).find((one) => one.url && !one.url.includes("node_modules")) ?? {};
              since.push(`${event.name.slice(0, 8)} ${data.nodeName ?? ""} ${data.reason ?? ""}${data.changedClass ? ` class ${data.changedClass}` : ""}${data.changedAttribute ? ` attribute ${data.changedAttribute}` : ""}${data.changedPseudo ? ` pseudo ${data.changedPseudo}` : ""}${data.invalidatedSelectorId ? "" : ""} @${(frame.url ?? "").split("/").slice(-2).join("/").replace(/\?.*$/, "")}:${frame.lineNumber ?? ""} ${frame.functionName ?? ""}`);
            }
          }
          const why = new Map();
          for (const event of traced) {
            if (event.name !== "StyleRecalcInvalidationTracking" && event.name !== "ScheduleStyleInvalidationTracking" && event.name !== "StyleInvalidatorInvalidationTracking") continue;
            const data = event.args?.data ?? {};
            const key = `${event.name.replace("InvalidationTracking", "")} ${data.nodeName ?? ""} ${data.reason ?? ""} ${data.extraData ?? ""} ${data.invalidationSet ? "" : ""}${data.changedClass ? `class ${data.changedClass}` : ""}${data.changedAttribute ? `attribute ${data.changedAttribute}` : ""}${data.changedId ? `id ${data.changedId}` : ""}${data.changedPseudo ? `pseudo ${data.changedPseudo}` : ""}`;
            why.set(key, (why.get(key) ?? 0) + 1);
          }
          for (const [key, count] of [...why].sort((a, b) => b[1] - a[1]).slice(0, 30)) console.log(`      ${count} ${key.slice(0, 200)}`);
        }
        const frames = await evaluate(`window.__glided.stop()`);
        // The glide: from the last frame before the sheet moved to the
        // first after which nothing moves again.
        const there = (box) => box && (box[2] > 0 || box[3] > 0);
        const moved = (a, b) => ["sheet", "panes", "scroller", "column", "words", "composer", "head"].some((name) => there(a[name]) && there(b[name]) && a[name].some((value, index) => Math.abs(value - b[name][index]) > 0.05));
        let first = frames.findIndex((frame, index) => index > 0 && moved(frames[index - 1], frame));
        if (first < 1) {
          console.log(`  ${act}: nothing moved (${frames.length} frames); laying out ${ms(after, before, "LayoutDuration")} ms, styles ${ms(after, before, "RecalcStyleDuration")} ms, script ${ms(after, before, "ScriptDuration")} ms, in tasks ${ms(after, before, "TaskDuration")} ms`);
          bad += 1;
          continue;
        }
        let last = frames.length - 1;
        while (last > first && !moved(frames[last - 1], frames[last])) last -= 1;
        const glide = frames.slice(first - 1, last + 1);
        const spans = glide.slice(1).map((frame, index) => frame.t - glide[index].t);
        const sorted = [...spans].sort((a, b) => a - b);
        const steps = (name, axis) => glide.slice(1).map((frame, index) => (there(frame[name]) && there(glide[index][name]) ? frame[name][axis] - glide[index][name][axis] : 0));
        const most = (values) => values.reduce((max, value) => (Math.abs(value) > Math.abs(max) ? value : max), 0);
        const wordsWide = steps("words", 2);
        const sheetMoved = part === "sidebar" ? steps("sheet", 0) : steps("sheet", 0).map((value, index) => value || steps("sheet", 1)[index]);
        // Between the column's region and the sheet: how far apart their
        // facing edges stand, frame by frame.
        const apart = glide.map((frame) => {
          if (!there(frame.sheet) || !there(frame.panes)) return 0;
          if (part === "sidebar") return frame.panes[0] - (frame.sheet[0] + frame.sheet[2]);
          if (dock === "bottom") return frame.sheet[1] - (frame.panes[1] + frame.panes[3]);
          if (dock === "left") return frame.panes[0] - (frame.sheet[0] + frame.sheet[2]);
          return frame.sheet[0] - (frame.panes[0] + frame.panes[2]);
        });
        const whole = (name, axis) => (glide[0][name] && glide[glide.length - 1][name] ? glide[glide.length - 1][name][axis] - glide[0][name][axis] : 0);
        const firstShare = whole("words", 2) ? wordsWide[0] / whole("words", 2) : 0;
        console.log(`  ${act}: ${glide.length - 1} frames over ${(glide[glide.length - 1].t - glide[0].t).toFixed(0)} ms; frame ${sorted[Math.floor(sorted.length / 2)].toFixed(1)} ms in the middle, ${sorted[sorted.length - 1].toFixed(1)} ms at most, ${spans.filter((span) => span > 25).length} over 25 ms`);
        console.log(`    laying out ${ms(after, before, "LayoutDuration")} ms, styles ${ms(after, before, "RecalcStyleDuration")} ms, script ${ms(after, before, "ScriptDuration")} ms, in tasks ${ms(after, before, "TaskDuration")} ms`);
        console.log(`    words: ${whole("words", 2).toFixed(0)} px wider in all, ${wordsWide[0].toFixed(1)} px at the first frame (${(firstShare * 100).toFixed(0)}%), ${most(wordsWide).toFixed(1)} px the largest step; left edge ${whole("words", 0).toFixed(1)} px in all, ${most(steps("words", 0)).toFixed(1)} px the largest step`);
        console.log(`    sheet: ${most(sheetMoved).toFixed(1)} px the largest step; from the column's region ${Math.min(...apart).toFixed(1)} to ${Math.max(...apart).toFixed(1)} px; composer ${most(steps("composer", 2)).toFixed(1)} px, head ${most(steps("head", 0)).toFixed(1)} px the largest steps; foot ${glide[0].foot} -> ${glide[glide.length - 1].foot}, ${Math.max(...glide.map((frame) => Math.abs(frame.foot - glide[0].foot))).toFixed(1)} px off at most`);
        console.log(`    the conversations' room: ${most(steps("panes", 2)).toFixed(1)} px across, ${most(steps("panes", 3)).toFixed(1)} px down the largest steps; ${whole("panes", 2).toFixed(0)} px across and ${whole("panes", 3).toFixed(0)} px down in all`);
        console.log(`    the words in view: ${whole("words", 1).toFixed(1)} px down in all, ${most(steps("words", 1)).toFixed(1)} px the largest step`);
        if (Math.abs(firstShare) > 0.34 && Math.abs(whole("words", 2)) > 8) bad += 1;
        if (all) for (const frame of glide) console.log(`      ${(frame.t - glide[0].t).toFixed(0).padStart(4)}ms sheet ${frame.sheet?.[0]},${frame.sheet?.[1]} panes ${frame.panes?.[0]},${frame.panes?.[1]}+${frame.panes?.[2]}x${frame.panes?.[3]} scroller ${frame.scroller?.[0]}+${frame.scroller?.[2]} words ${frame.words?.[0]},${frame.words?.[1]}+${frame.words?.[2]}x${frame.words?.[3]} composer ${frame.composer?.[0]},${frame.composer?.[1]}+${frame.composer?.[2]}x${frame.composer?.[3]} head ${frame.head?.[0]} view ${frame.scroller?.[3]} top ${frame.top} of ${frame.tall} foot ${frame.foot}`);
        await sleep(400);
      }
    }
  }
  if (problems.length) console.log(`problems: ${JSON.stringify([...new Set(problems)].slice(0, 8))}`);
  console.log(bad ? `${bad} glides took their room at once or did not move` : "every glide gave its room a little at a time");
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
