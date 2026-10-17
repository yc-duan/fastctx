// Checks the interface against its design rules (`AGENTS.md`, 前端设计), to
// run by hand after a change: colours, shadows, durations and easings come from the
// design tokens (`src/design/tokens.css`), never written in a component's
// styles; the interface's words never string facts on middle dots; a class
// name is declared in one stylesheet only, unless it is the same component's.
//
//   node scripts/check-design.mjs [--all]
//
// Prints each finding as file:line and what it is; exits 1 when there are
// any. `--all` also lists repeated class names across stylesheets.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const root = new URL("../src/", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const all = process.argv.includes("--all");

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path, out);
    else out.push(path);
  }
  return out;
}

const files = walk(root);
const findings = [];
const say = (path, line, what) => findings.push(`${relative(root, path).replaceAll("\\", "/")}:${line}: ${what}`);

// Styles: no literal colour, shadow or duration outside the tokens.
const colour = /#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(/;
const duration = /\b(?!0m?s\b)\d*\.?\d+m?s\b/;
for (const path of files.filter((one) => one.endsWith(".css"))) {
  // The tokens, and the syntax colours that are a theme of their own.
  if (path.endsWith("tokens.css") || path.endsWith("highlight.css")) continue;
  const lines = readFileSync(path, "utf8").split("\n");
  let comment = false;
  lines.forEach((line, index) => {
    let text = line;
    if (comment) {
      const end = text.indexOf("*/");
      if (end < 0) return;
      text = text.slice(end + 2);
      comment = false;
    }
    text = text.replace(/\/\*.*?\*\//g, "");
    const open = text.indexOf("/*");
    if (open >= 0) {
      text = text.slice(0, open);
      comment = true;
    }
    const value = text.split(":").slice(1).join(":");
    if (!value) return;
    if (colour.test(value) && !/url\(/.test(value) && !/^\s*(-webkit-)?mask/.test(text)) say(path, index + 1, `a colour of its own: ${text.trim()}`);
    if (/^\s*(transition|animation)/.test(text)) {
      const literal = value.replace(/var\([^)]*\)/g, "");
      if (duration.test(literal.replace(/cubic-bezier\([^)]*\)/g, ""))) say(path, index + 1, `a duration of its own: ${text.trim()}`);
      if (/cubic-bezier\(/.test(literal)) say(path, index + 1, `an easing of its own: ${text.trim()}`);
    }
    if (/^\s*box-shadow/.test(text) && !/var\(--/.test(value) && !/\bnone\b|\btransparent\b/.test(value) && !/inset 0 0 0/.test(value)) say(path, index + 1, `a shadow of its own: ${text.trim()}`);
  });
}

// Words: no fact strung on a middle dot, written in words or set between
// spans. Specimens and stand-ins may; so may Claude's own words quoted,
// the drawing's handle, whose mark is a dot, and a gauge's typing noise.
for (const path of files.filter((one) => one.endsWith(".ts") && !/specimen/.test(one))) {
  readFileSync(path, "utf8")
    .split("\n")
    .forEach((line, index) => {
      if (/^\s*(\/\/|\*)/.test(line) || !line.includes("·")) return;
      if (/Please run \/login|textContent = "·"|talk-gauge-noise/.test(line)) return;
      say(path, index + 1, `a middle dot: ${line.trim().slice(0, 100)}`);
    });
}

// Custom properties a stylesheet uses that nothing defines: a typo there
// fails without a word. The pen's rules are set by script (`pen/rules.ts`).
{
  const defined = new Set();
  for (const path of files.filter((one) => /\.(css|ts)$/.test(one))) {
    const text = readFileSync(path, "utf8");
    for (const match of text.matchAll(/(--[\w-]+)\s*:/g)) defined.add(match[1]);
    for (const match of text.matchAll(/["'`](--[\w-]+)["'`]/g)) defined.add(match[1]);
  }
  for (const path of files.filter((one) => one.endsWith(".css"))) {
    readFileSync(path, "utf8")
      .split("\n")
      .forEach((line, index) => {
        for (const match of line.matchAll(/var\((--[\w-]+)/g)) {
          if (!defined.has(match[1]) && !/^--pen-(rule|swash|underline)/.test(match[1])) say(path, index + 1, `${match[1]} is never defined`);
        }
      });
  }
}

// Class names declared in more than one stylesheet.
if (all) {
  const owners = new Map();
  for (const path of files.filter((one) => one.endsWith(".css"))) {
    for (const match of readFileSync(path, "utf8").matchAll(/^\.([a-z][a-z0-9-]*)\s*[{,]/gm)) {
      const seen = owners.get(match[1]) ?? new Set();
      seen.add(relative(root, path).replaceAll("\\", "/"));
      owners.set(match[1], seen);
    }
  }
  for (const [name, seen] of owners) if (seen.size > 1) findings.push(`.${name}: declared in ${[...seen].join(", ")}`);
}

for (const finding of findings) console.log(finding);
console.log(`${findings.length} finding${findings.length === 1 ? "" : "s"}`);
process.exit(findings.length ? 1 : 0);
