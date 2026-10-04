// Screenshots of the interface for looking at it: every specimen at a
// phone's, a tablet's and a desktop's width, in each theme, whole pages.
// Drives a headless Chromium over the DevTools protocol; the dev server
// must be running (`npm run dev`).
//
//   node scripts/shoot.mjs [--url http://localhost:5317] [--only claude] [--out shots]

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const at = args.indexOf(`--${name}`);
  return at === -1 ? fallback : args[at + 1];
};
const base = option("url", "http://localhost:5317");
const out = option("out", "shots");
const only = option("only", null);
const extra = option("query", "");

const browsers = [
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
];
const browser = browsers.find((path) => existsSync(path));
if (!browser) throw new Error("no Chromium found");

const widths = [
  { name: "phone", width: 390, height: 844, scale: 3, mobile: true },
  { name: "tablet", width: 820, height: 1180, scale: 2, mobile: true },
  { name: "desktop", width: 1280, height: 860, scale: 1.5, mobile: false },
];
const themes = ["warm-black", "ink-black", "warm-white"];
const sessions = only ? [only] : ["home", "claude", "codex", "project", "journal"];

const profile = mkdtempSync(join(tmpdir(), "shoot-"));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn(browser, [
  "--headless=new",
  `--remote-debugging-port=${port}`,
  `--user-data-dir=${profile}`,
  "--no-first-run",
  "--no-default-browser-check",
  "--hide-scrollbars",
  "--force-color-profile=srgb",
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

await send("Page.enable");
await send("Runtime.enable");
const problems = [];
listeners.add((data) => {
  if (data.method === "Runtime.exceptionThrown") problems.push(data.params.exceptionDetails.exception?.description ?? "exception");
  if (data.method === "Runtime.consoleAPICalled" && data.params.type === "error") {
    problems.push(data.params.args.map((arg) => arg.value ?? arg.description).join(" "));
  }
});

mkdirSync(out, { recursive: true });
for (const session of sessions) {
  for (const size of widths) {
    for (const theme of themes) {
      await send("Emulation.setDeviceMetricsOverride", {
        width: size.width,
        height: size.height,
        deviceScaleFactor: size.scale,
        mobile: size.mobile,
      });
      await send("Emulation.setTouchEmulationEnabled", { enabled: size.mobile });
      const loaded = once("Page.loadEventFired");
      await send("Page.navigate", { url: `${base}/?s=${session}&theme=${theme}${extra ? `&${extra}` : ""}` });
      await loaded;
      // Fonts, then any diagram, then two frames for layout to settle.
      await send("Runtime.evaluate", {
        expression:
          "document.fonts.ready.then(() => new Promise(r => setTimeout(r, 1500))).then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))",
        awaitPromise: true,
      });
      const { result } = await send("Runtime.evaluate", {
        expression: "JSON.stringify({ w: document.documentElement.scrollWidth, h: document.documentElement.scrollHeight })",
        returnByValue: true,
      });
      const page = JSON.parse(result.value);
      if (page.w > size.width) {
        // Name the innermost elements that reach past the screen's edge.
        const { result: wide } = await send("Runtime.evaluate", {
          expression: `JSON.stringify([...document.querySelectorAll("body *")]
            .filter((e) => e.getBoundingClientRect().right > innerWidth + 1 || e.scrollWidth > e.clientWidth + 1 && getComputedStyle(e).overflowX === "visible")
            .filter((e) => ![...e.children].some((c) => c.getBoundingClientRect().right > innerWidth + 1))
            .slice(0, 6)
            .map((e) => e.tagName.toLowerCase() + "." + [...e.classList].join(".") + " " + Math.round(e.getBoundingClientRect().right) + " " + (e.textContent || "").slice(0, 60)))`,
          returnByValue: true,
        });
        problems.push(`${session} ${size.name} ${theme}: the page is ${page.w}px wide: ${wide.value}`);
      }
      // The screen made as tall as the page, so what sticks to its edges
      // sits where it would at either end.
      await send("Runtime.evaluate", { expression: "scrollTo(0, 0)" });
      await send("Emulation.setDeviceMetricsOverride", {
        width: size.width,
        height: page.h,
        deviceScaleFactor: size.scale,
        mobile: size.mobile,
      });
      const { result: settled } = await send("Runtime.evaluate", {
        expression:
          "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(document.documentElement.scrollHeight))))",
        awaitPromise: true,
        returnByValue: true,
      });
      const shot = await send("Page.captureScreenshot", {
        format: "png",
        clip: { x: 0, y: 0, width: size.width, height: settled.value, scale: 1 },
        captureBeyondViewport: true,
      });
      const file = join(out, `${session}-${size.name}-${theme}.png`);
      writeFileSync(file, Buffer.from(shot.data, "base64"));
      console.log(`${file} (${size.width}×${page.h})`);
    }
  }
}

if (problems.length) console.log(`\nproblems:\n${[...new Set(problems)].join("\n")}`);
socket.close();
chrome.kill();
await new Promise((resolve) => setTimeout(resolve, 300));
rmSync(profile, { recursive: true, force: true });
