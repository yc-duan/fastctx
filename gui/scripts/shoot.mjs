// Screenshots of the interface for looking at it: every specimen at a
// phone's, a tablet's and a desktop's size, in each theme, one screen each,
// as the interface is meant to be seen. A page that scrolls as a whole or
// reaches past the screen's edge is named among the problems.
// Drives a headless Chromium over the DevTools protocol; the dev server
// must be running (`npm run dev`).
//
//   node scripts/shoot.mjs [--url http://localhost:5317] [--only claude] [--theme warm-black] [--width phone] [--out shots] [--probe <expression>]
//
// `--probe` evaluates an expression in each scene once it has settled and
// prints what it returns, for measuring a layout rather than eyeing it.
//
// `drawer` is the front page with the sidebar pulled out as a drawer;
// `about` is a conversation with its head's "⋯" open; a name
// ending in `-2` is its page logged in to two accounts on two hubs.

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
const probe = option("probe", null);

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
const sessions = only ? only.split(",") : ["welcome", "create", "created", "home", "claude", "codex", "project", "journal", "machines", "settings", "new", "new-project", "invite", "invite-windows", "invite-windows-made", "share", "lend", "relays", "palette", "login", "account", "admin", "home-2", "machines-2", "new-2", "slash", "drawer", "about", "manage", "asks", "asks-question", "asks-form", "asks-link", "asks-claude-permission", "asks-claude-question", "asks-claude-plan", "asks-claude-form", "stop", "new-codex", "new-attach", "new-lent-2", "new-pick-2", "find", "attach", "queue", "rewind", "usage", "service", "remove-keep", "remove-erase", "remove-run", "mention", "decision", "log", "written", "page", "page-ask", "page-versions"];
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
socket.addEventListener("close", () => {
  for (const { reject } of pending.values()) reject(new Error("Chromium's DevTools connection closed"));
  pending.clear();
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
// The browser's own errors: a resource the content policy refused, one
// that did not load.
await send("Log.enable");
const problems = [];
listeners.add((data) => {
  if (data.method === "Runtime.exceptionThrown") problems.push(data.params.exceptionDetails.exception?.description ?? "exception");
  // A browser asks for a favicon on its own; the shell never does.
  if (data.method === "Log.entryAdded" && data.params.entry.level === "error" && !data.params.entry.url?.endsWith("/favicon.ico")) problems.push(`${data.params.entry.text} ${data.params.entry.url ?? ""}`.trim());
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
      const two = session.endsWith("-2");
      const named = two ? session.slice(0, -2) : session;
      const shown = named.startsWith("invite-") ? "invite" : named === "stop" ? "claude" : named === "drawer" || named === "palette" ? "home" : named === "service" || named.startsWith("remove-") ? "settings" : named === "slash" || named === "mention" || named === "find" || named === "attach" || named === "rewind" || named === "usage" ? "codex" : named.startsWith("asks") ? "asks" : named === "new-codex" || named === "new-attach" || named === "new-lent" || named === "new-pick" ? "new" : named === "manage" ? "about" : named === "welcome" || named === "create" || named === "created" ? "login" : named === "share" || named === "relays" ? "machines" : named === "lend" || named.startsWith("ledger") ? "home" : named === "queue" ? "codex" : named.startsWith("page") ? "page" : named.startsWith("panel") ? "claude" : named.startsWith("team") ? "stream" : named.startsWith("repair-") ? "home" : named === "updates-repair" ? "machines" : named === "journal-edit" ? "journal" : named === "project-write" ? "project" : named === "features" || named.startsWith("feature-") ? "home" : named;
      // `welcome` and `create` are a computer's first open, in no World yet.
      const alone = named === "welcome" || named === "create" || named === "created";
      const ask = (session.startsWith("asks-") ? `&ask=${session.slice(5)}` : "") + (two ? "&accounts=2" : "") + (alone ? "&accounts=0" : "") + (named.startsWith("team") ? "&start=agents" : "") + (named.startsWith("repair-") ? `&repair=${named === "repair-broken" ? "broken" : "failed"}` : "");
      // A page an agent wrote is its own document, with a stand-in host.
      const written = session === "written";
      const specimen = named === "mapcanvas" ? "mapcanvas-specimen.html" : named === "longview" ? "longview-specimen.html" : written ? "page-specimen.html" : null;
      // `startup-failed` and the like are the page shown until Fastctx opens.
      const startup = named.startsWith("startup-") ? named.slice("startup-".length) : null;
      // `side-goal-blocked` is the panel beside a conversation at its goal
      // page, the goal blocked; `side-plan` its plan.
      const side = named.startsWith("side-") ? named.slice("side-".length).split("-") : null;
      const url = side
        ? `${base}/side-specimen.html?page=${side[0]}${side[1] ? `&goal=${side[1]}` : ""}&theme=${theme}${extra ? `&${extra}` : ""}`
        : specimen
        ? `${base}/${specimen}?theme=${theme}${extra ? `&${extra}` : ""}`
        : startup
          ? `${base}/?startup=${startup}&theme=${theme}${extra ? `&${extra}` : ""}`
          : `${base}/?s=${shown}${ask}&theme=${theme}${extra ? `&${extra}` : ""}`;
      await send("Page.navigate", { url });
      await loaded;
      // The interface loads after the engine check: wait until it is there.
      await send("Runtime.evaluate", {
        awaitPromise: true,
        expression: written
          ? "new Promise((r) => { const look = () => (customElements.get('w-card') ? setTimeout(r, 900) : setTimeout(look, 30)); look(); })"
          : startup
            ? "new Promise((r) => { const look = () => (document.querySelector('.su-page') ? setTimeout(r, 600) : setTimeout(look, 30)); look(); })"
            : side
              ? "new Promise((r) => { const look = () => (document.querySelector('.sp-panel') ? setTimeout(r, 900) : setTimeout(look, 30)); look(); })"
            : "new Promise((r) => { const look = () => (document.querySelector('.shell') ? r(true) : setTimeout(look, 30)); look(); })",
      });
      if (session === "drawer") {
        await send("Runtime.evaluate", { expression: "document.querySelector('.menu-button')?.click()" });
      }
      // An open page asks first what it may do; the frame then draws.
      if (session.startsWith("page")) {
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `new Promise((r) => setTimeout(r, 1200)).then(() => {
            if (${JSON.stringify(session)} !== "page-ask") document.querySelector(".page-sheet .sheet-close")?.click();
            if (${JSON.stringify(session)} === "page-versions") document.querySelector(".page-versions")?.click();
            return new Promise((r) => setTimeout(r, 900));
          })`,
        });
      }
      // `features` is the features' cards; `feature-memory` and the like one
      // feature's page.
      if (named === "features" || named.startsWith("feature-")) {
        const where = named === "features" ? "#/features" : `#/feature/${named.slice("feature-".length)}`;
        await send("Runtime.evaluate", { awaitPromise: true, expression: `new Promise((r) => { location.hash = ${JSON.stringify(where)}; setTimeout(r, 2600); })` });
      }
      // `journal-edit` is a decision being edited; `project-write` the sheet
      // for writing a journal (both with `--query memory`).
      if (named === "journal-edit" || named === "project-write") {
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `new Promise((r) => setTimeout(() => { document.querySelector(${JSON.stringify(named === "journal-edit" ? ".mm-edit" : "[data-write-journal]")})?.click(); setTimeout(r, 700); }, 700))`,
        });
      }
      // `updates-repair` is the machines' update view with two machines picked,
      // its bar offering to repair them.
      if (named === "updates-repair") {
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `new Promise((r) => { location.hash = "#/updates"; setTimeout(() => { document.querySelector("[data-ml-pick-all]")?.click(); setTimeout(r, 900); }, 1200); })`,
        });
      }
      // `repair-offered` is this device's update card where the update did not
      // go through; `repair-going`, `repair-done` and `repair-broken` a repair
      // pressed there, under way, ended well, and stopped short.
      if (named.startsWith("repair-")) {
        const wait = named === "repair-going" ? 2200 : named === "repair-offered" ? 0 : 6500;
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `new Promise((r) => { document.querySelector(".update-place")?.click(); setTimeout(() => { ${named === "repair-offered" ? "" : "document.querySelector('[data-update-repair]')?.click();"} setTimeout(r, ${wait} + 900); }, 600); })`,
        });
      }
      // `team` is a conversation whose host sent agents, the panel open at
      // who works with it; `team-open` with the first of them opened from
      // the row that sent it.
      if (named.startsWith("team")) {
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `new Promise((r) => setTimeout(r, 2500)).then(() => new Promise((r) => { ${
            named === "team-open" ? "const head = [...document.querySelectorAll('.group-head')].filter((one) => one.getAttribute('aria-expanded') !== 'true').pop(); head?.click(); [...document.querySelectorAll('.row-agent .row-line')].pop()?.click();" : "document.querySelector('.sp-toggle')?.click(); document.querySelector('.sp-tab[data-page=\"collab\"]')?.click();"
          } setTimeout(r, 1200); }))`,
        });
      }
      // `panel` is a conversation with the panel beside it open; `panel-goal`
      // and `panel-collab` open it at that page.
      if (named.startsWith("panel")) {
        const tab = named.slice("panel".length).replace(/^-/, "");
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `new Promise((r) => { document.querySelector(".sp-toggle")?.click(); ${tab ? `document.querySelector('.sp-tab[data-page="${tab}"]')?.click();` : ""} setTimeout(r, 900); })`,
        });
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
      // A turn under way, the moment stop is pressed: its specimen never
      // stops, so it stays as the press shows it.
      if (session === "stop") {
        await send("Runtime.evaluate", { awaitPromise: true, expression: "new Promise(r => setTimeout(r, 300)).then(() => { document.querySelector('.composer-action.stop')?.click(); return new Promise(r => setTimeout(r, 500)); })" });
      }
      if (session === "rewind") {
        await send("Runtime.evaluate", { expression: "[...document.querySelectorAll('.turn-rewind')].pop()?.click()" });
      }
      if (session === "usage") {
        await send("Runtime.evaluate", { expression: "new Promise(r => setTimeout(r, 200)).then(() => document.querySelector('.composer-context')?.click())", awaitPromise: true });
      }
      if (session === "new-codex") {
        await send("Runtime.evaluate", { expression: "new Promise(r => setTimeout(r, 300)).then(() => document.querySelector('[data-choice=pick][data-value=\"desktop|codex|\"]')?.click())", awaitPromise: true });
      }
      if (named === "new-lent") {
        // An agent lent by another account: asked no machine to work on.
        await send("Runtime.evaluate", {
          expression: "new Promise(r => setTimeout(r, 600)).then(() => { document.querySelector('[data-choice=expand]')?.click(); return new Promise(r => setTimeout(r, 100)); }).then(() => document.querySelector('.picker-lent .login-choice')?.click())",
          awaitPromise: true,
        });
      }
      if (named === "new-pick") {
        // Choosing another login: every one, as the user holds them.
        await send("Runtime.evaluate", { expression: "new Promise(r => setTimeout(r, 600)).then(() => document.querySelector('[data-choice=expand]')?.click())", awaitPromise: true });
      }
      if (session === "new-attach") {
        // A picture and a log dropped on the sheet wait there; beginning
        // sends them first, then asks the agent in (a specimen stops there).
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `(async () => {
            await new Promise((r) => setTimeout(r, 300));
            const canvas = document.createElement("canvas");
            canvas.width = 320; canvas.height = 200;
            const g = canvas.getContext("2d");
            const grad = g.createLinearGradient(0, 0, 320, 200);
            grad.addColorStop(0, "#7fa8cf"); grad.addColorStop(1, "#d97757");
            g.fillStyle = grad; g.fillRect(0, 0, 320, 200);
            const png = await new Promise((r) => canvas.toBlob(r, "image/png"));
            const dt = new DataTransfer();
            dt.items.add(new File([png], "bode-plot.png", { type: "image/png" }));
            dt.items.add(new File(["x".repeat(64000)], "tuning-run-07.log", { type: "text/plain" }));
            document.querySelector(".new-talk-sheet form").dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
            const t = document.querySelector(".first-words");
            t.value = "照着这张伯德图和这份调参日志，把相位裕度拉回 45 度以上";
            t.dispatchEvent(new Event("input"));
            document.querySelector(".new-talk-sheet .sheet-go").click();
            await new Promise((r) => setTimeout(r, 2400));
          })()`,
        });
      }
      if (session === "share" || session === "relays") {
        // Sharing lab, and the relays lab connects through. lab is pointed
        // at on the drawing first; the panel shows this computer until then.
        const button = `[data-machine="lab"] [data-act="${session}"]`;
        await send("Runtime.evaluate", { awaitPromise: true, expression: `new Promise(r => setTimeout(r, 300)).then(() => { document.querySelector('.map-node[data-key="m:lab"]')?.click(); return new Promise(r => setTimeout(r, 900)); }).then(() => document.querySelector('${button}')?.click())` });
      }
      if (session.startsWith("ledger")) {
        // The desktop Claude login's usage page: as it opens; ada, who has
        // allowances, picked; the people past the first few unfolded; ada
        // picked and then bo, the pick moving.
        const pickOne = (name) => `[...document.querySelectorAll('.ledger-person[data-only]')].find((b) => b.textContent.includes('${name}'))?.click();`;
        const steps = { "ledger-ada": [pickOne("ada")], "ledger-all": ["document.querySelector('.ledger-others')?.click();"], "ledger-switch": [pickOne("ada"), pickOne("bo")] }[session] ?? [];
        const then = steps.map((step) => `.then(() => { ${step} return new Promise(r => setTimeout(r, 500)); })`).join("");
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `new Promise(r => { location.hash = '#/agents/desktop/claude/' + encodeURIComponent(['C:', 'Users', 'me', '.claude'].join(String.fromCharCode(92))); setTimeout(r, 1500); })${then}`,
        });
      }
      if (session === "lend") {
        // A Claude login of this computer lent: its assist sheet from the
        // login's menu, with a weekly limit typed and the Fable limit under it.
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: "new Promise(r => { location.hash = '#/agents/claude'; setTimeout(r, 900); }).then(() => { document.querySelector('.logins-more')?.click(); return new Promise(r => setTimeout(r, 300)); }).then(() => { document.querySelector('[data-act=\"assist\"]')?.click(); return new Promise(r => setTimeout(r, 700)); }).then(() => { const w = document.querySelector('.assist-new [name=weekly]'); if (w) { w.value = '100'; w.dispatchEvent(new Event('input', { bubbles: true })); } return new Promise(r => setTimeout(r, 200)); })",
        });
      }
      if (session === "create" || session === "created") {
        await send("Runtime.evaluate", { awaitPromise: true, expression: "new Promise(r => setTimeout(r, 300)).then(() => document.querySelector('[data-way=create]')?.click())" });
      }
      if (session === "created") {
        // Filled in and made: the page that lets the next computer in.
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: "new Promise(r => setTimeout(r, 500)).then(() => { const set = (n, v) => { const f = document.querySelector(`[name=${n}]`); f.value = v; }; set('world', 'home'); set('account', 'me'); set('password', 'Quiet-harbor-7'); set('again', 'Quiet-harbor-7'); document.querySelector('.create-form').requestSubmit(); return new Promise(r => setTimeout(r, 1800)); })",
        });
      }
      // A managed Windows computer's line: the way chosen, then the line made.
      if (session === "invite-windows" || session === "invite-windows-made") {
        const made = session === "invite-windows-made" ? "return new Promise(r => setTimeout(r, 300)).then(() => document.querySelector('.token-sheet .sheet-go')?.click()).then(() => new Promise(r => setTimeout(r, 600)));" : "";
        await send("Runtime.evaluate", { awaitPromise: true, expression: `new Promise(r => setTimeout(r, 200)).then(() => { document.querySelector('.token-sheet [data-face=managed]')?.click(); return new Promise(r => setTimeout(r, 200)); }).then(() => { document.querySelector('.token-sheet [data-managed-way=windows]')?.click(); ${made} })` });
      }
      if (session === "invite") {
        await send("Runtime.evaluate", { awaitPromise: true, expression: "new Promise(r => setTimeout(r, 200)).then(() => document.querySelector('.token-sheet .sheet-go')?.click())" });
      }
      if (session === "mention") {
        // A file being mentioned: `@` and a few letters of its name.
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: "new Promise(r => setTimeout(r, 200)).then(() => { const t = document.querySelector('.composer textarea'); t.focus(); t.value = '看一下 @pid'; t.setSelectionRange(t.value.length, t.value.length); t.dispatchEvent(new Event('input')); return new Promise(r => setTimeout(r, 400)); })",
        });
      }
      if (session === "service") {
        // The startup service missing, then installed.
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: "new Promise(r => setTimeout(r, 200)).then(() => { document.querySelector('[data-service=keep]')?.click(); return new Promise(r => setTimeout(r, 1300)); })",
        });
      }
      if (session.startsWith("remove-")) {
        // Taking fastctx off the computer: keeping the data; everything,
        // with its list open; everything, done step by step.
        const way = session === "remove-keep" ? "keep" : "erase";
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `(async () => {
            const pause = (ms) => new Promise((r) => setTimeout(r, ms));
            await pause(300);
            document.querySelector('[data-removal=${way}]').click();
            await pause(800);
            if (${JSON.stringify(session)} === "remove-erase") document.querySelector('[data-removal=list]').click();
            if (${JSON.stringify(session)} === "remove-run") {
              const name = document.querySelector('[data-removal-name]');
              name.value = 'lab';
              name.dispatchEvent(new Event('input', { bubbles: true }));
              document.querySelector('[data-removal=go]').click();
              document.querySelector('[data-removal=go]').click();
              await pause(5200);
            }
            document.querySelector('.removal').scrollIntoView({ block: 'start' });
            await pause(500);
          })()`,
        });
      }
      if (session === "about") {
        await send("Runtime.evaluate", { expression: "document.querySelector('.talk-menu-toggle')?.click()" });
      }
      if (session === "queue") {
        // Words said while the agent works: two wait their turn.
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: `(async () => {
            await new Promise((r) => setTimeout(r, 1300));
            const t = document.querySelector(".composer textarea");
            const say = (text) => { t.value = text; t.dispatchEvent(new Event("input")); document.querySelector(".composer-action").click(); };
            say("先别动 writer.rs，等我看完这份日志再说");
            await new Promise((r) => setTimeout(r, 300));
            say("看完之后把 retry 的测试在 lab 上也跑一遍，结果贴给我");
            await new Promise((r) => setTimeout(r, 300));
          })()`,
        });
      }
      if (session === "join") {
        // The way into a World with a code, pasted or (on a phone) scanned.
        await send("Runtime.evaluate", { awaitPromise: true, expression: "new Promise(r => setTimeout(r, 300)).then(() => document.querySelector('[data-path=join]')?.click())" });
      }
      if (session === "manage") {
        // The card open at its management: the projects to move it to shown,
        // then putting it to sleep answered (a specimen says it is one).
        await send("Runtime.evaluate", {
          awaitPromise: true,
          expression: "new Promise(r => setTimeout(r, 200)).then(() => { document.querySelector('.talk-menu-toggle')?.click(); return new Promise(r => setTimeout(r, 600)); }).then(() => { document.querySelector('[data-act=move]')?.click(); document.querySelector('[data-act=sleep]')?.click(); return new Promise(r => setTimeout(r, 1200)); })",
        });
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
      // A page an agent wrote scrolls inside the frame the host gives it.
      if (!written && page.h > page.inner + 1) problems.push(`${session} ${size.name} ${theme}: the page scrolls, ${page.h}px in a ${page.inner}px screen`);
      if (probe) {
        const { result: probed } = await send("Runtime.evaluate", { expression: `JSON.stringify(${probe})`, returnByValue: true });
        console.log(`${session} ${size.name} ${theme}: ${probed.value}`);
      }
      const shot = await send("Page.captureScreenshot", { format: "png" });
      const file = join(out, `${session}-${size.name}-${theme}.png`);
      writeFileSync(file, Buffer.from(shot.data, "base64"));
      console.log(`${file} (${size.width}×${size.height})`);
    }
  }
}

if (problems.length) console.log(`\nproblems:\n${[...new Set(problems)].join("\n")}`);
const closed = new Promise((resolve) => chrome.once("exit", resolve));
const closing = send("Browser.close").then(() => null, (error) => error);
const code = await closed;
const closeError = await closing;
socket.close();
if (code !== 0) throw new Error(`Chromium shutdown exited ${code}: ${closeError ?? ""}`);
if (closeError) console.log("Chromium exited normally before acknowledging Browser.close");
rmSync(profile, { recursive: true, force: true });
