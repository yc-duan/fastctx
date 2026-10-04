// Drives a built shell without showing it: starts the executable with
// WebView2's remote debugging open, reads the page over the DevTools
// protocol, takes screenshots, and stops it. Read-only: it never says
// anything to a resident.
//
//   node scripts/probe-shell.mjs <shell.exe> [hash route ...]

import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";

const [exe, ...routes] = process.argv.slice(2);
const port = 9333;
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("CLAUDE")));
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${port}`;
const shell = spawn(exe, [], { env, stdio: ["ignore", "pipe", "pipe"] });
let log = "";
shell.stdout.on("data", (chunk) => (log += chunk));
shell.stderr.on("data", (chunk) => (log += chunk));

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let target;
for (let attempt = 0; attempt < 150 && !target; attempt += 1) {
  try {
    const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    target = list.find((entry) => entry.type === "page");
  } catch {
    await sleep(200);
  }
}
if (!target) {
  console.log("no page target; shell output:\n" + log);
  shell.kill();
  process.exit(1);
}
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve) => socket.addEventListener("open", resolve, { once: true }));
let serial = 0;
const pending = new Map();
const problems = [];
socket.addEventListener("message", (message) => {
  const data = JSON.parse(message.data);
  if (data.id && pending.has(data.id)) {
    pending.get(data.id)(data);
    pending.delete(data.id);
  } else if (data.method === "Runtime.exceptionThrown") {
    problems.push(data.params.exceptionDetails.exception?.description ?? data.params.exceptionDetails.text);
  } else if (data.method === "Runtime.consoleAPICalled" && data.params.type === "error") {
    problems.push(data.params.args.map((arg) => arg.value ?? arg.description).join(" "));
  }
});
const send = (method, params = {}) =>
  new Promise((resolve) => {
    serial += 1;
    pending.set(serial, resolve);
    socket.send(JSON.stringify({ id: serial, method, params }));
  });
const evaluate = async (expression) =>
  (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result?.result?.value;

await send("Runtime.enable");
await send("Page.enable");
mkdirSync("shots", { recursive: true });
for (const [index, route] of ["#/", ...routes].entries()) {
  await evaluate(`location.hash = ${JSON.stringify(route)}`);
  await sleep(4500);
  const text = await evaluate("document.querySelector('#app')?.innerText.slice(0, 1500)");
  console.log(`--- ${route}\n${text}\n`);
  const shot = await send("Page.captureScreenshot", { format: "png" });
  if (shot.result?.data) writeFileSync(`shots/shell-${index}.png`, Buffer.from(shot.result.data, "base64"));
}
if (problems.length) console.log("problems:\n" + [...new Set(problems)].join("\n"));
socket.close();
shell.kill();
