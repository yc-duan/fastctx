// Screenshots of the interface for looking at it: every specimen at a
// phone's, a tablet's and a desktop's size, in each theme, one screen each,
// as the interface is meant to be seen. A page that scrolls as a whole or
// reaches past the screen's edge is named among the problems.
// Drives a headless Chromium over the DevTools protocol; the dev server
// must be running (`npm run dev`).
//
//   node scripts/shoot.mjs [--url http://localhost:5317] [--only claude] [--theme warm-black] [--width phone] [--out shots]
//
// `drawer` is the front page with the sidebar pulled out as a drawer;
// `about` is a conversation with the card about its agent open.

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
const themes = option("theme", null) ? [option("theme")] : ["warm-black", "ink-black", "warm-white"];
const sessions = only ? only.split(",") : ["home", "claude", "codex", "project", "journal", "machines", "settings", "new", "new-project", "invite", "palette", "welcome", "slash", "drawer", "about", "asks", "asks-question", "asks-form", "asks-link", "new-codex", "find", "attach", "rewind", "usage", "service", "mention", "decision", "log"];
const sizes = option("width", null) ? widths.filter((size) => option("width").split(",").includes(size.name)) : widths;

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
  for (const size of sizes) {
    for (const theme of themes) {
      await send("Emulation.setDeviceMetricsOverride", {
        width: size.width,
        height: size.height,
        deviceScaleFactor: size.scale,
        mobile: size.mobile,
      });
      await send("Emulation.setTouchEmulationEnabled", { enabled: size.mobile });
      const loaded = once("Page.loadEventFired");
      const shown = session === "drawer" || session === "palette" ? "home" : session === "service" ? "settings" : session === "slash" || session === "mention" || session === "find" || session === "attach" || session === "rewind" || session === "usage" ? "codex" : session.startsWith("asks") ? "asks" : session === "new-codex" ? "new" : session;
      const ask = session.startsWith("asks-") ? `&ask=${session.slice(5)}` : "";
      await send("Page.navigate", { url: `${base}/?s=${shown}${ask}&theme=${theme}${extra ? `&${extra}` : ""}` });
      await loaded;
      // The interface loads after the engine check: wait until it is there.
      await send("Runtime.evaluate", {
        awaitPromise: true,
        expression: "new Promise((r) => { const look = () => (document.querySelector('.shell') ? r(true) : setTimeout(look, 30)); look(); })",
      });
      if (session === "drawer") {
        await send("Runtime.evaluate", { expression: "document.querySelector('.menu-button')?.click()" });
      }
      if (session === "palette") {
        await send("Runtime.evaluate", { expression: "dispatchEvent(new KeyboardEvent('keydown', { key: 'k', ctrlKey: true }))" });
      }
      if (session === "slash") {
        await send("Runtime.evaluate", {
          expression: "(() => { const t = document.querySelector('.composer textarea'); t.focus(); t.value = '/'; t.dispatchEvent(new Event('input')); })()",
        });
      }
      if (session === "find") {
        await send("Runtime.evaluate", {
          expression: "(() => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true })); const i = document.querySelector('.find-input'); i.value = 'commit'; i.dispatchEvent(new Event('input')); })()",
        });
      }
      if (session === "attach") {
        // Files made in the page and dropped on it: two arrive and go with
        // words, then one more is still on its way.
        const made = await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `(async () => {
            const canvas = document.createElement("canvas");
            canvas.width = 320; canvas.height = 200;
            const g = canvas.getContext("2d");
            const grad = g.createLinearGradient(0, 0, 320, 200);
            grad.addColorStop(0, "#d97757"); grad.addColorStop(1, "#7fa8cf");
            g.fillStyle = grad; g.fillRect(0, 0, 320, 200);
            const png = await new Promise((r) => canvas.toBlob(r, "image/png"));
            const drop = (files) => {
              const dt = new DataTransfer();
              for (const f of files) dt.items.add(f);
              document.querySelector(".conversation-page").dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
            };
            drop([new File([png], "step-response.png", { type: "image/png" }), new File(["x".repeat(48000)], "train.log", { type: "text/plain" })]);
            await new Promise((r) => setTimeout(r, 1600));
            const t = document.querySelector(".composer textarea");
            t.value = "看看这张图和这份日志，step_response 为什么停在 0.96？";
            t.dispatchEvent(new Event("input"));
            document.querySelector(".composer-action").click();
            drop([new File(["y".repeat(9000000)], "capture.pcap", { type: "application/octet-stream" }), new File([png], "截图 2026-10-04.png", { type: "image/png" })]);
            await new Promise((r) => setTimeout(r, 300));
            await new Promise((r) => setTimeout(r, 1500));
          })()`,
        });
        if (made.exceptionDetails) problems.push(`attach: ${made.exceptionDetails.exception?.description ?? made.exceptionDetails.text}`);
      }
      if (session === "rewind") {
        await send("Runtime.evaluate", { expression: "[...document.querySelectorAll('.turn-rewind')].pop()?.click()" });
      }
      if (session === "usage") {
        await send("Runtime.evaluate", { expression: "new Promise(r => setTimeout(r, 200)).then(() => document.querySelector('.composer-context')?.click())", awaitPromise: true });
      }
      if (session === "new-codex") {
        await send("Runtime.evaluate", { expression: "new Promise(r => setTimeout(r, 300)).then(() => document.querySelector('[data-choice=agent][data-value=codex]')?.click())", awaitPromise: true });
      }
      if (session === "invite") {
        await send("Runtime.evaluate", { expression: "document.querySelector('.device-sheet .sheet-go')?.click()" });
      }
      if (session === "mention") {
        // A file being mentioned: `@` and a few letters of its name.
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: "new Promise(r => setTimeout(r, 200)).then(() => { const t = document.querySelector('.composer textarea'); t.focus(); t.value = '看一下 @pid'; t.setSelectionRange(t.value.length, t.value.length); t.dispatchEvent(new Event('input')); return new Promise(r => setTimeout(r, 400)); })",
        });
      }
      if (session === "service") {
        // The startup service installed, then removal armed by one press.
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: "new Promise(r => setTimeout(r, 200)).then(() => { document.querySelector('[data-service=keep]')?.click(); return new Promise(r => setTimeout(r, 1300)); }).then(() => document.querySelector('[data-service=release]')?.click())",
        });
      }
      if (session === "about") {
        await send("Runtime.evaluate", { expression: "document.querySelector('.details-toggle')?.click()" });
      }
      // Fonts, then any diagram, then two frames for layout to settle.
      await send("Runtime.evaluate", {
        expression:
          "document.fonts.ready.then(() => new Promise(r => setTimeout(r, 1500))).then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))",
        awaitPromise: true,
      });
      const { result } = await send("Runtime.evaluate", {
        expression: "JSON.stringify({ w: document.documentElement.scrollWidth, h: document.documentElement.scrollHeight, inner: innerHeight })",
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
      if (page.h > page.inner + 1) problems.push(`${session} ${size.name} ${theme}: the page scrolls, ${page.h}px in a ${page.inner}px screen`);
      const shot = await send("Page.captureScreenshot", { format: "png" });
      const file = join(out, `${session}-${size.name}-${theme}.png`);
      writeFileSync(file, Buffer.from(shot.data, "base64"));
      console.log(`${file} (${size.width}×${size.height})`);
    }
  }
}

if (problems.length) console.log(`\nproblems:\n${[...new Set(problems)].join("\n")}`);
socket.close();
chrome.kill();
await new Promise((resolve) => setTimeout(resolve, 300));
rmSync(profile, { recursive: true, force: true });
