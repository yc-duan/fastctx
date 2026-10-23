// Tests as their runners print them: Rust's test harness (`cargo test`)
// and pytest. Counts come from the runners' own totals where they gave
// them, from the tests' own lines while a run is still going or where the
// totals were left out.

import type { Found, OutputLines, TestFailure, TestsDeed } from "./types";

const TEST = /^test (.+?) \.\.\. (ok|FAILED|ignored)\b/;
const RESULT = /^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;/;
const RUNNING = /^running \d+ tests?$/;
const STDOUT = /^---- (.+?) stdout ----$/;

/** The tests Rust's harness ran, all of them one deed. */
export function rustTests(lines: OutputLines): Found<TestsDeed> | null {
  let at = -1;
  let passed = 0;
  let failed = 0;
  let ignored = 0;
  // What the lines of the block under way have counted, until its total.
  let block = { passed: 0, failed: 0, ignored: 0 };
  const failures = new Map<string, TestFailure>();
  let telling: TestFailure | null = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) {
      telling = null;
      continue;
    }
    const stdout = STDOUT.exec(line);
    if (stdout) {
      telling = failures.get(stdout[1]!) ?? { name: stdout[1]!, said: [] };
      failures.set(telling.name, telling);
      continue;
    }
    if (line === "failures:" || RESULT.test(line)) telling = null;
    if (telling) {
      telling.said.push(line);
      continue;
    }
    if (RUNNING.test(line)) {
      if (at < 0) at = index;
      continue;
    }
    const test = TEST.exec(line);
    if (test) {
      if (at < 0) at = index;
      if (test[2] === "ok") block.passed += 1;
      else if (test[2] === "ignored") block.ignored += 1;
      else {
        block.failed += 1;
        if (!failures.has(test[1]!)) failures.set(test[1]!, { name: test[1]!, said: [] });
      }
      continue;
    }
    const result = RESULT.exec(line);
    if (result) {
      if (at < 0) at = index;
      passed += Number(result[1]);
      failed += Number(result[2]);
      ignored += Number(result[3]);
      block = { passed: 0, failed: 0, ignored: 0 };
    }
  }
  if (at < 0) return null;
  passed += block.passed;
  failed += block.failed;
  ignored += block.ignored;
  for (const failure of failures.values()) trimEnd(failure.said);
  return { at, deed: { kind: "tests", command: null, passed, failed, ignored, failures: [...failures.values()] } };
}

const SUMMARY = /^=+ (.+?) in [\d.]+s\b.* =+$/;
const COUNT = /(\d+) (passed|failed|skipped|errors?|xfailed|xpassed|deselected|warnings?)\b/g;
const PYTEST_FAILED = /^(?:FAILED|ERROR) (\S+)(?: - (.*))?$/;

/** The tests pytest ran, from its last line and its short summary. */
export function pytestTests(lines: OutputLines): Found<TestsDeed> | null {
  let at = -1;
  let passed = 0;
  let failed = 0;
  let ignored = 0;
  const failures: TestFailure[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (!line) continue;
    const failedLine = PYTEST_FAILED.exec(line);
    if (failedLine) {
      if (at < 0) at = index;
      failures.push({ name: failedLine[1]!, said: failedLine[2] ? [failedLine[2]] : [] });
      continue;
    }
    const summary = SUMMARY.exec(line);
    if (!summary || !/\b(?:passed|failed|errors?|skipped)\b/.test(summary[1]!)) continue;
    if (at < 0) at = index;
    for (const count of summary[1]!.matchAll(COUNT)) {
      const n = Number(count[1]);
      if (count[2] === "passed" || count[2] === "xpassed") passed += n;
      else if (count[2] === "failed" || count[2]!.startsWith("error")) failed += n;
      else if (count[2] === "skipped" || count[2] === "xfailed") ignored += n;
    }
  }
  if (at < 0) return null;
  return { at, deed: { kind: "tests", command: null, passed, failed, ignored, failures } };
}

function trimEnd(lines: string[]): void {
  while (lines.length && !lines[lines.length - 1]!.trim()) lines.pop();
  while (lines.length && !lines[0]!.trim()) lines.shift();
}
