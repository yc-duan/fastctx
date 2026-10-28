// What a deed's frame is named by: the package or the folder tests and
// checks ran in, and a command without what only routes its output.

import { wordsOf } from "./script";
import type { TestsDeed } from "./types";

/** What tests were run: the package and filter named, or the folder they
 *  ran in. */
export function testSubject(deed: TestsDeed): string {
  let words = wordsOf(deed.command ?? "");
  if (words[0] === "on" && words.length > 2) words = words.slice(2);
  // A package manager's script, or a runner it fetched: what it ran.
  if (["npm", "pnpm", "yarn", "bun", "npx", "bunx"].includes(words[0] ?? "")) {
    const rest = words.slice(1).filter((word) => !["test", "run", "exec", "dlx", "x", "--"].includes(word) && !word.startsWith("-"));
    const named = rest.filter((word) => !["jest", "vitest"].includes(word));
    return named[0] ?? (deed.cwd ? leaf(deed.cwd) : words.slice(0, 2).join(" "));
  }
  if (words[0] === "go") {
    const named = words.slice(2).filter((word) => !word.startsWith("-"));
    return named.join(" ") || (deed.cwd ? leaf(deed.cwd) : "go test");
  }
  if (words[0] === "jest" || words[0] === "vitest") {
    const named = words.slice(1).filter((word) => !word.startsWith("-") && word !== "run");
    return named[0] ?? (deed.cwd ? leaf(deed.cwd) : words[0]);
  }
  const named: string[] = [];
  let index = words[0] === "cargo" ? 2 : words[0] === "pytest" ? 1 : words.indexOf("pytest") + 1;
  if (words[0] === "cargo" && words[1] === "nextest" && words[2] === "run") index = 3;
  for (; index < words.length; index += 1) {
    const word = words[index]!;
    if (word === "--") break;
    if (word === "-p" || word === "--package") {
      if (words[index + 1]) named.push(words[index + 1]!);
      index += 1;
      continue;
    }
    if (word.startsWith("-")) {
      if (/^-(?:k|m|j|-features|-test|-bin|-target)$/.test(word)) index += 1;
      continue;
    }
    named.push(word);
  }
  return named.join(" ") || (deed.cwd ? leaf(deed.cwd) : words.slice(0, 2).join(" "));
}

/** What a cargo command checked or built: its tool, the package it named,
 *  or the folder it ran in. */
export function cargoSubject(command: string | null, cwd: string | null): string {
  let words = wordsOf(command ?? "");
  if (words[0] === "on" && words.length > 2) words = words.slice(2);
  if (words[0] !== "cargo") {
    // tsc, ESLint and the scripts that run them: the tool, and what it was
    // pointed at.
    const rest = words.filter((word, index) => index > 0 && !word.startsWith("-") && !["run", "exec", "dlx", "x"].includes(word));
    const tool = ["npx", "bunx", "npm", "pnpm", "yarn", "bun"].includes(words[0] ?? "") ? (rest.shift() ?? words[0] ?? "") : (words[0] ?? "");
    const target = rest.find((word) => word !== "." && !/^(?:tsconfig.*\.json)$/.test(word));
    return [tool, target].filter(Boolean).join(" ") || (cwd ? leaf(cwd) : "");
  }
  const tool = words[1] === "clippy" ? "clippy" : "";
  const at = words.findIndex((word) => word === "-p" || word === "--package");
  const named = at >= 0 ? (words[at + 1] ?? "") : "";
  return [tool, named || (cwd ? leaf(cwd) : "")].filter(Boolean).join(" ") || words.slice(0, 2).join(" ");
}

/** A command without what only routes its output: where its errors go,
 *  the last lines of it kept, a copy written aside. */
export function shortCommand(command: string): string {
  return command
    .replace(/\s+\d?>&\d/g, "")
    .replace(/\s+(?:\d|&)?>>?\s*\/dev\/null/g, "")
    .replace(/\s*\|\s*(?:tail|head)(?:\s+-n?\s*\d+|\s+-\d+)?\s*$/g, "")
    .replace(/\s*\|\s*tee\s+\S+\s*$/g, "")
    .trim();
}

function leaf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
}
