// Tests as the other common runners print them: Jest, Vitest and Go's
// own. Counts come from the runners' totals; what each failure printed
// comes from the block the runner gives it. Go without `-v` names only
// the failing tests: its packages are counted instead, one each.

import type { Found, OutputLines, TestFailure, TestsDeed } from "./types";

const JEST_TOTAL = /^Tests:\s+(.+?)\s*$/;
const JEST_FAILURE = /^ {2}● (.+)$/;
const JEST_FILE = /^(?:PASS|FAIL) \S/;
const COUNT = /(\d+) (passed|failed|skipped|todo|pending)\b/g;

/** What Jest ran, from its `Tests:` total and its `●` failures. */
export function jestTests(lines: OutputLines): Found<TestsDeed> | null {
  let at = -1;
  let total: { passed: number; failed: number; ignored: number } | null = null;
  const failures: TestFailure[] = [];
  let telling: TestFailure | null = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) continue;
    const failure = JEST_FAILURE.exec(line);
    if (failure) {
      if (at < 0) at = index;
      telling = { name: failure[1]!.replace(/ › /g, " > "), said: [] };
      failures.push(telling);
      continue;
    }
    const counted = JEST_TOTAL.exec(line);
    if (counted && /\d+ (?:passed|failed)/.test(counted[1]!)) {
      if (at < 0) at = index;
      total = counts(counted[1]!);
      telling = null;
      continue;
    }
    if (JEST_FILE.test(line) || /^Test Suites:/.test(line)) {
      telling = null;
      continue;
    }
    if (telling) telling.said.push(line.replace(/^ {4}/, ""));
  }
  if (at < 0 || !total) return null;
  for (const failure of failures) trim(failure.said);
  return { at, deed: { kind: "tests", command: null, ...total, failures } };
}

const VITEST_TOTAL = /^\s*Tests\s+(.+?)\s*\(\d+\)\s*$/;
const VITEST_FAILURE = /^\s*FAIL\s+(.+?)\s*$/;
const VITEST_RULE = /^\s*⎯/;

/** What Vitest ran, from its `Tests` total and its `FAIL` blocks. */
export function vitestTests(lines: OutputLines): Found<TestsDeed> | null {
  let at = -1;
  let total: { passed: number; failed: number; ignored: number } | null = null;
  const failures: TestFailure[] = [];
  let telling: TestFailure | null = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) continue;
    const counted = VITEST_TOTAL.exec(line);
    if (counted && /\b(?:passed|failed)\b/.test(counted[1]!)) {
      if (at < 0) at = index;
      total = counts(counted[1]!.replace(/\|/g, " "));
      telling = null;
      continue;
    }
    const failure = VITEST_FAILURE.exec(line);
    if (failure && failure[1]!.includes(" > ")) {
      if (at < 0) at = index;
      telling = { name: failure[1]!, said: [] };
      failures.push(telling);
      continue;
    }
    if (VITEST_RULE.test(line) || /^\s*Test Files\s/.test(line)) {
      telling = null;
      continue;
    }
    if (telling) telling.said.push(line);
  }
  if (at < 0 || !total) return null;
  for (const failure of failures) trim(failure.said);
  return { at, deed: { kind: "tests", command: null, ...total, failures } };
}

const GO_TEST = /^\s*--- (PASS|FAIL|SKIP): (\S+)/;
const GO_PACKAGE = /^(ok|FAIL|---)\s+(\S+)\s+(?:[\d.]+s|\(cached\)|\[)/;

/** What `go test` ran: its tests where it named them (`-v`), else its
 *  packages; what each failing test logged under its line. */
export function goTests(lines: OutputLines): Found<TestsDeed> | null {
  let at = -1;
  const tests = { passed: 0, failed: 0, ignored: 0 };
  const packages = { passed: 0, failed: 0, ignored: 0 };
  let named = false;
  const failures: TestFailure[] = [];
  let telling: TestFailure | null = null;
  // With `-v` a test logs between its `=== RUN` and its result.
  let logged: string[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) continue;
    if (/^=== RUN\s/.test(line)) {
      logged = [];
      telling = null;
      continue;
    }
    const test = GO_TEST.exec(line);
    if (test) {
      if (at < 0) at = index;
      telling = null;
      if (test[1] === "PASS") {
        tests.passed += 1;
        named = true;
      } else if (test[1] === "SKIP") tests.ignored += 1;
      else {
        tests.failed += 1;
        telling = { name: test[2]!, said: logged };
        failures.push(telling);
      }
      logged = [];
      continue;
    }
    const pack = GO_PACKAGE.exec(line);
    if (pack && pack[1] !== "---") {
      if (at < 0) at = index;
      telling = null;
      if (pack[1] === "ok") packages.passed += 1;
      else packages.failed += 1;
      continue;
    }
    if (/^(?:PASS|FAIL)\b/.test(line)) {
      telling = null;
      continue;
    }
    if (telling) telling.said.push(line.trim());
    else logged.push(line.trim());
  }
  if (at < 0) return null;
  const counted = named ? tests : { ...packages, failed: Math.max(packages.failed, tests.failed && !packages.failed ? 1 : 0) };
  return { at, deed: { kind: "tests", command: null, ...counted, failures } };
}

function counts(said: string): { passed: number; failed: number; ignored: number } {
  const total = { passed: 0, failed: 0, ignored: 0 };
  for (const count of said.matchAll(COUNT)) {
    const n = Number(count[1]);
    if (count[2] === "passed") total.passed += n;
    else if (count[2] === "failed") total.failed += n;
    else total.ignored += n;
  }
  return total;
}

function trim(lines: string[]): void {
  while (lines.length && !lines[lines.length - 1]!.trim()) lines.pop();
  while (lines.length && !lines[0]!.trim()) lines.shift();
}
