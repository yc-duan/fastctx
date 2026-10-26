// Two sessions written the way the World sends them, for looking at the
// interface without a world behind it: every kind of thing a conversation
// shows, from both hosts, in the shapes the node really produces.

import { requestEvents } from "./requests";
import { run, Script, sent, text } from "./script";
import { steps } from "./steps";
import { trips } from "./trips";
import { deeds } from "./deeds";
import type { SessionEvent, Speaker } from "../world/events";
import type { JsonValue } from "../world/wire";
import { cut, hunk, leftOut, noted, printed, put, ran, same, wrote } from "./run-facts";
import { tour } from "./tour";
import { voice } from "./voice";

export interface Specimen {
  title: string;
  agent: "claude" | "codex";
  resident: string;
  machine: string;
  model: string;
  town: string;
  here: string;
  events: SessionEvent[];
}

const user: Speaker = { kind: "user", machine: "desktop" };

function claude(): Specimen {
  const s = new Script("2026-10-04T13:02:00Z");

  s.heard("t1", user, "lab 上 ctrl 的测试一直挂在 step_response，帮我看看是怎么回事，顺手修掉。修完在 lab 上把整套测试跑一遍。", 4, undefined, [
    sent("step-response.png", "image/png", 48_210),
    sent("scope-capture.png", "image/png", 131_884),
    sent("ctrl-test.log", "text/plain", 9_416),
  ]);
  s.started("t1");
  s.say("t1", "thinking", "", 2);
  s.tool(
    "t1",
    run(
      "rg -n 'fn step' /home/me/mark50/src/ctrl",
      "=== run on laptop (exited 0) ===",
      "/home/me/mark50/src/ctrl/pid.rs:41:    pub fn step(&mut self, error: f32, dt: f32) -> f32 {\n/home/me/mark50/src/ctrl/fixed.rs:58:    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {",
      { duration_ms: 84 },
    ),
  );
  s.tool(
    "t1",
    run(
      "fastctx view -- /home/me/mark50/src/ctrl/fixed.rs /home/me/mark50/src/ctrl/pid.rs",
      "=== run on laptop (exited 0) ===",
      "=== view (2 files: 2 shown) ===\n=== laptop:/home/me/mark50/src/ctrl/fixed.rs (lines 1-112 of 112) ===\n1\t//! Fixed-point PID for the joint controllers.\n2\t\n3\tuse crate::q16::Q16;\n…",
      { duration_ms: 120 },
    ),
  );
  // The whole test run printed four thousand lines; the model got its
  // first and last lines, the rest stays in its log on lab.
  const testHead = [
    "   Compiling ctrl v0.4.1 (/srv/mark50/ctrl)",
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 8.91s",
    "     Running unittests src/lib.rs (target/debug/deps/ctrl-5b1f0e2a9c4d7781)",
    "",
    "running 42 tests",
    ...Array.from({ length: 9 }, (_, n) => `test fixed::tests::case_${String(n + 1).padStart(3, "0")} ... ok`),
  ];
  const testTail = [
    "failures:",
    "",
    "---- fixed::tests::step_response stdout ----",
    "thread 'fixed::tests::step_response' panicked at ctrl/src/fixed.rs:131:9:",
    "assertion `left == right` failed: settled at 0.9612, wanted 1.0000 ± 0.002",
    "  left: 0.9612",
    " right: 1.0000",
    "",
    "failures:",
    "    fixed::tests::step_response",
    "",
    "test result: FAILED. 41 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
    "",
    "error: test failed, to rerun pass `-p ctrl --lib`",
  ];
  const testLines = 4000;
  s.tool(
    "t1",
    run(
      "cd /srv/mark50 && cargo test -p ctrl 2>&1",
      "=== run on lab (exited 101) ===",
      `${testHead.join("\n")}\n=== lines 15-3986 of 4000 not shown ===\n${testTail.join("\n")}\n=== all of its output is in /home/me/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-p0q1r2.log ===`,
      {
        duration_ms: 9400,
        run: ran(
          "lab",
          "j-p0q1r2",
          { kind: "ended", status: { code: 101 } },
          [printed(1, `${testHead.join("\n")}\n`), leftOut(15, testLines - testTail.length), printed(testLines - testTail.length + 1, `${testTail.join("\n")}\n`)],
          [],
          {
            lines: testLines,
            cut: true,
            make: () => [
              ...testHead,
              ...Array.from({ length: testLines - testHead.length - testTail.length }, (_, n) => `test fixed::tests::case_${String(n + 10).padStart(4, "0")} ... ok`),
              ...testTail,
            ],
          },
        ),
      },
      "lab",
    ),
  );
  s.say("t1", "thinking", "", 3);
  // One run that only wrote: its writes are its rows.
  const vectors = [
    "// Step-response vectors recorded on lab, 2026-10-04: input, expected output.",
    "pub const STEP: [(i32, i32); 640] = [",
    ...Array.from({ length: 640 }, (_, n) => `    (${n * 65_536}, ${Math.round(65_536 * (1 - Math.exp(-n / 90)))}),`),
    "];",
  ];
  s.tool(
    "t1",
    run(
      [
        "edit /home/me/mark50/src/ctrl/fixed.rs <<'EOF'",
        "<<<<<<< SEARCH",
        "    integral: Q16,",
        "=======",
        "    integral: Q32,",
        ">>>>>>> REPLACE",
        "<<<<<<< SEARCH",
        "        self.integral += error * dt;",
        "        let i = self.ki * self.integral;",
        "=======",
        "        // Accumulate in Q32: a slow error is otherwise lost to rounding.",
        "        self.integral = self.integral.saturating_add(error.widen() * dt.widen());",
        "        let i = (self.ki.widen() * self.integral).narrow();",
        ">>>>>>> REPLACE",
        "EOF",
        "write /home/me/mark50/src/ctrl/tests/step_vectors.rs <<'EOF'",
        ...vectors.slice(0, 4),
        "…",
        "EOF",
      ].join("\n"),
      "=== run on laptop (exited 0) ===",
      "=== edit /home/me/mark50/src/ctrl/fixed.rs (2 changes) ===\n=== write /home/me/mark50/src/ctrl/tests/step_vectors.rs (created) ===",
      {
        duration_ms: 140,
        run: ran(
          "laptop",
          "j-e41c07",
          { kind: "ended", status: { code: 0 } },
          [
            noted("edit", "7c1e2a90", 1, "=== edit /home/me/mark50/src/ctrl/fixed.rs (2 changes) ===\n"),
            noted("write", "9b04f1d3", 2, "=== write /home/me/mark50/src/ctrl/tests/step_vectors.rs (created) ===\n"),
          ],
          [
            wrote("7c1e2a90", 0, "laptop", "/home/me/mark50/src/ctrl/fixed.rs", "modified", [
              hunk(12, 12),
              same(12, 12, "    ki: Q16,"),
              same(13, 13, "    kd: Q16,"),
              cut(14, "    integral: Q16,"),
              put(14, "    integral: Q32,"),
              same(15, 15, "    last: Q16,"),
              same(16, 16, "}"),
              hunk(39, 39),
              same(39, 39, "    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {"),
              same(40, 40, "        let p = self.kp * error;"),
              cut(41, "        self.integral += error * dt;"),
              cut(42, "        let i = self.ki * self.integral;"),
              put(41, "        // Accumulate in Q32: a slow error is otherwise lost to rounding."),
              put(42, "        self.integral = self.integral.saturating_add(error.widen() * dt.widen());"),
              put(43, "        let i = (self.ki.widen() * self.integral).narrow();"),
              same(43, 44, "        let d = self.kd * (error - self.last) / dt;"),
              same(44, 45, "        self.last = error;"),
            ]),
            wrote("9b04f1d3", 0, "laptop", "/home/me/mark50/src/ctrl/tests/step_vectors.rs", "added", [hunk(1, 1), ...vectors.map((line, n) => put(n + 1, line))]),
          ],
        ),
      },
    ),
  );
  const goingLog = [
    "   Compiling ctrl v0.4.1 (/srv/mark50/ctrl)",
    "   Compiling joints v0.4.1 (/srv/mark50/joints)",
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 14.20s",
    "     Running unittests src/lib.rs (target/debug/deps/joints-0c9e4b21d7f3a556)",
  ];
  s.tool(
    "t1",
    run(
      "cd /srv/mark50 && cargo test --workspace",
      "=== run on lab (still running after 15s; j-k3x9a2) ===",
      "   Compiling ctrl v0.4.1 (/srv/mark50/ctrl)\n   Compiling joints v0.4.1 (/srv/mark50/joints)\n=== j-k3x9a2 goes on in the background; all of its output is in /home/me/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-k3x9a2.log ===",
      {
        duration_ms: 15000,
        run: ran("lab", "j-k3x9a2", { kind: "running" }, [printed(1, `${goingLog.slice(0, 2).join("\n")}\n`)], [], { lines: 2, cut: false, make: () => [...goingLog] }),
      },
      "lab",
    ),
  );
  s.say(
    "t1",
    "message",
    `找到了。\`fixed.rs\` 里的积分项是用 Q16 累加的，每一步 $e \\cdot \\Delta t$ 都会被舍入到 $2^{-16}$；误差越接近零，被舍掉的比例越大，所以系统停在 0.9612，再也爬不上去。

### 改了什么

- 积分改成在 **Q32** 里累加，只在乘上 $K_i$ 之后才收窄回 Q16；
- 累加用 \`saturating_add\`，长时间饱和时不会回绕。

稳态误差的来源可以写成

$$
e_{ss} \\approx \\frac{2^{-16}}{K_i \\, \\Delta t}
$$

在 lab 的参数下（$K_i = 0.8$，$\\Delta t = 1\\,\\text{ms}$）大约是 $0.019$，和测到的偏差对得上。

| 参数 | 改前 | 改后 |
|---|---|---|
| 积分精度 | Q16（$2^{-16}$） | Q32（$2^{-32}$） |
| 稳态值 | 0.9612 | 0.9999 |
| 每步开销 | 3 次乘法 | 4 次乘法 |

\`\`\`rust
let i = (self.ki.widen() * self.integral).narrow();
\`\`\`

整套测试已经在 lab 上跑起来了，大约要三分钟，跑完我会接着看结果。`,
    3,
  );
  s.push({ event: "context", used: 61_400, window: 200_000 });
  s.ended("t1", "completed", null, 1, {
    ms: 41_000,
    facts: {
      models: ["claude-opus-5-5"],
      efforts: [{ kind: "requested", value: "high" }],
      tokens: { host: "claude", input_tokens: 2_318, cache_read_input_tokens: 41_872, cache_creation_input_tokens: 3_904, output_tokens: 1_266 },
      cost: 421_337,
      limits: {
        observed_at: "2026-10-04T13:06:40Z",
        windows: [
          { name: "seven_day", minutes: 10080, used_tenths: 641, resets_at: "2026-10-08T18:00:00Z" },
          { name: "five_hour", minutes: 300, used_tenths: 412, resets_at: "2026-10-04T17:00:00Z" },
        ],
      },
    },
  });

  s.heard(
    "t2",
    { kind: "world" },
    "=== j-k3x9a2 on lab ended (exited 0) at 2026-10-04 13:09:12 UTC; its output is in /home/me/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-k3x9a2.log ===",
    190,
    {
      kind: "job_ended",
      job: "j-k3x9a2",
      machine: "lab",
      how: { kind: "finished" },
      status: { code: 0 },
      at: "2026-10-04T13:09:12Z",
      output: "/home/me/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-k3x9a2.log",
      last: {
        text: "test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.12s\n\n   Doc-tests ctrl\ntest result: ok. 3 passed; 0 failed; 0 ignored",
        whole: false,
      },
    },
  );
  s.started("t2");
  s.say("t2", "thinking", "", 2);
  s.tool(
    "t2",
    run(
      "tail -4 /home/me/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-k3x9a2.log",
      "=== run on lab (exited 0) ===",
      "test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.12s\n\n   Doc-tests ctrl\ntest result: ok. 3 passed; 0 failed; 0 ignored",
      { duration_ms: 60 },
      "lab",
    ),
  );
  s.tool("t2", {
    name: "ask_colleague",
    machine: "laptop",
    input: {
      description: "造出让 Q32 积分溢出的输入",
      prompt:
        "We moved the PID integral in /home/me/mark50/src/ctrl/fixed.rs to Q32 with saturating_add. Make it fail: find inputs where the integral or the narrowed output wraps instead of saturating, with a test that reproduces it.",
    },
    output: text(
      "=== colleague codex-91c0 on laptop (started) ===\nIts last message of each turn will be delivered to you. See what it is doing: fastctx w codex-91c0",
    ),
    does: { as: "colleague", to: "codex-91c0", task: "造出让 Q32 积分溢出的输入" },
  });
  s.say(
    "t2",
    "message",
    "整套测试在 lab 上全过了：42 个单元测试、3 个文档测试，用时 3 分 12 秒。\n\n我另请了一位同事去造让新积分溢出的输入。收窄回 Q16 的那一步最可疑，它交回来我再告诉你。",
    3,
  );
  s.ended("t2", "completed");

  // The bench machine was taken out of the network while a benchmark ran
  // there: its end can no longer come, and the stop sent after it could not
  // reach it either. The World says so, once each.
  const gone = "The end of j-b7m2q5 on bench cannot be delivered: bench is no longer in this account's current network view.";
  const unstopped = "The stop on bench could not be delivered: bench is no longer in this account's current network view.";
  s.heard("t3", { kind: "world" }, gone, 600, { kind: "world", fact: { fact: "delivery_failed", id: "delivery-failed-6c1e0a93b2d4f5a7c8e9" }, text: gone });
  s.heard("t3", { kind: "world" }, unstopped, 1, { kind: "world", fact: { fact: "delivery_failed", id: "delivery-failed-0b7d2e4f6a8c1e3d5f7a" }, text: unstopped });

  s.heard(
    "t3",
    { kind: "resident", address: { machine: "laptop", resident: "codex-91c0" } },
    "=== colleague codex-91c0 on laptop (turn completed, 14m) ===\nFound one. With `ki = 0x7FFF_0000` and a constant error of 32767 at `dt = 1`, the integral saturates after 65 538 steps as meant, but `narrow()` then **wraps** instead of saturating: the output jumps from 32767 to −32768.\n\nRepro: `cargo test -p ctrl overflow_repro` on branch `colleague/q16-edge` (laptop, `/home/me/wt/q16-edge`).\n=== what it did: fastctx w codex-91c0 ===",
    240,
    {
      kind: "answer",
      colleague: { machine: "laptop", resident: "codex-91c0" },
      label: "GPT-6.1-Sol · laptop",
      turn: "c1",
      end: {
        how: "completed",
        reason: null,
        duration_ms: 14 * 60_000,
        last_message:
          "Found one. With `ki = 0x7FFF_0000` and a constant error of 32767 at `dt = 1`, the integral saturates after 65 538 steps as meant, but `narrow()` then **wraps** instead of saturating: the output jumps from 32767 to −32768.\n\nRepro: `cargo test -p ctrl overflow_repro` on branch `colleague/q16-edge` (laptop, `/home/me/wt/q16-edge`).",
      },
    },
  );
  s.started("t3");
  s.say("t3", "thinking", "", 2);
  // A run under way that has already written: its write is told while it
  // runs, and has its row before the run answers.
  const fixing = s.tool(
    "t3",
    run(
      "on laptop edit /home/me/mark50/src/ctrl/q16.rs <<'EOF'\n<<<<<<< SEARCH\n        Q16((self.0 >> 16) as i32)\n=======\n        Q16((self.0 >> 16).clamp(i32::MIN as i64, i32::MAX as i64) as i32)\n>>>>>>> REPLACE\nEOF\ncd /srv/mark50 && git fetch laptop colleague/q16-edge && cargo test -p ctrl overflow_repro",
      "",
      "",
      {},
      "lab",
    ),
    2,
    true,
  );
  s.push({
    event: "changed",
    turn: "t3",
    change: {
      kind: "run_facts_updated",
      call_id: fixing,
      update: {
        call_id: fixing,
        primary: false,
        output: null,
        visited: [["laptop", "j-5d20aa"]],
        machine: "laptop",
        job: "j-5d20aa",
        sequence: 1,
        state: null,
        writes: [
          wrote("c3a81f02", 0, "laptop", "/home/me/mark50/src/ctrl/q16.rs", "modified", [
            hunk(21, 21),
            same(21, 21, "    /// Back to Q16, keeping the integer part."),
            same(22, 22, "    pub fn narrow(self) -> Q16 {"),
            cut(23, "        Q16((self.0 >> 16) as i32)"),
            put(23, "        Q16((self.0 >> 16).clamp(i32::MIN as i64, i32::MAX as i64) as i32)"),
            same(24, 24, "    }"),
            same(25, 25, "}"),
          ]),
        ],
      },
    },
  });

  return {
    title: "ctrl 的 step_response",
    agent: "claude",
    resident: "claude-2b1c",
    machine: "laptop",
    model: "Opus 5.5",
    town: "mark50",
    here: "desktop",
    events: [...s.recent(), ...requestEvents({ agent: "claude", count: 64, from: 14_200, to: 61_280, cold: [21], compact: 44, compacted: 0.55, turn: "t2" })],
  };
}

function codex(): Specimen {
  const s = new Script("2026-10-04T08:40:00Z");

  // A screenshot pasted straight into Codex: the World never kept it, its
  // thread's own record holds it.
  s.heard("t1", user, "ingest 服务现在每个文件提交一次，大文件会把事务拖得很长。改成每 5000 条一批提交，先给我个计划。", 4, undefined, undefined, [
    { source: { owner: { machine: "laptop", resident: "codex-7f3a" }, host_image: { host: "codex", turn: "t1", message: 0, image: 0 } }, name: null, media_type: "image/png" },
  ]);
  s.started("t1");
  s.say(
    "t1",
    "thinking",
    "**Reading the ingest writer**\n\nThe writer opens one transaction per file and commits at EOF. Batching means the commit moves into `flush()`, and the retry path has to replay only the open batch.",
    2,
  );
  s.tool(
    "t1",
    run(
      "fastctx view -- /srv/ingest/src/writer.rs /srv/ingest/src/retry.rs",
      "=== run on laptop (exited 0) ===",
      "=== view (2 files: 2 shown) ===\n…",
      { duration_ms: 90 },
    ),
  );
  s.tool(
    "t1",
    run(
      "rg -n 'commit|begin_tx' /srv/ingest/src",
      "=== run on laptop (exited 0) ===",
      "/srv/ingest/src/writer.rs:77:        let tx = self.db.begin_tx()?;\n/srv/ingest/src/writer.rs:112:        tx.commit()?;\n/srv/ingest/src/retry.rs:23:    // A failed commit replays the whole file.",
      { duration_ms: 70 },
    ),
  );
  s.say(
    "t1",
    "plan",
    "1. Move the transaction into `Writer::flush`, opened lazily on the first row of a batch.\n2. Commit every 5 000 rows and at end of file; keep the row count in the writer, not the caller.\n3. Make `retry.rs` replay only the rows since the last commit, using the batch's first offset.\n4. Add a test that kills the writer mid-batch and checks no row is written twice.",
    3,
  );
  s.ended("t1", "completed");

  s.heard("t2", user, "可以，按这个来。", 60);
  s.started("t2");
  s.push({
    event: "steps",
    turn: "t2",
    note: null,
    steps: [
      { text: "Move the transaction into Writer::flush", status: "done" },
      { text: "Commit every 5 000 rows and at end of file", status: "done" },
      { text: "Replay only the open batch on retry", status: "doing" },
      { text: "Test a kill in the middle of a batch", status: "pending" },
    ],
  });
  s.say(
    "t2",
    "thinking",
    "**Patching the writer**\n\nThe batch counter lives next to the open transaction so a commit and its reset cannot drift apart.",
    2,
  );
  s.tool("t2", {
    name: "apply_patch",
    machine: "laptop",
    input: { input: "*** Begin Patch\n…" },
    output: text("=== apply_patch on laptop (1 file) ===\nM /srv/ingest/src/writer.rs"),
    does: {
      as: "edit",
      changes: [
        {
          path: "/srv/ingest/src/writer.rs",
          how: "updated",
          diff: [
            "@@ impl Writer {",
            "     pub fn push(&mut self, row: Row) -> Result<()> {",
            "-        self.rows.push(row);",
            "-        Ok(())",
            "+        let tx = self.tx.get_or_insert_with(|| self.db.begin_tx())?;",
            "+        tx.insert(&row)?;",
            "+        self.pending += 1;",
            "+        if self.pending == BATCH {",
            "+            self.flush()?;",
            "+        }",
            "+        Ok(())",
            "     }",
            "@@ pub fn flush(&mut self) -> Result<()> {",
            "-        let tx = self.db.begin_tx()?;",
            "-        for row in self.rows.drain(..) {",
            "-            tx.insert(&row)?;",
            "-        }",
            "-        tx.commit()",
            "+        if let Some(tx) = self.tx.take() {",
            "+            tx.commit()?;",
            "+        }",
            "+        self.pending = 0;",
            "+        Ok(())",
            "",
          ].join("\n"),
        },
      ],
    },
  });
  s.tool("t2", {
    name: "apply_patch",
    machine: "laptop",
    status: "failed",
    input: { input: "*** Begin Patch\n*** Update File: /srv/ingest/src/retry.rs\n…" },
    output: text(
      "=== apply_patch on laptop (failed) ===\n/srv/ingest/src/retry.rs: the context at hunk 1 was not found:\n    // A failed commit replays the whole file.\n    let start = 0;",
    ),
    does: { as: "edit", changes: [{ path: "/srv/ingest/src/retry.rs", how: "updated", diff: "@@\n-    let start = 0;\n+    let start = batch.first_offset;\n" }] },
  });
  s.push({ event: "changed", turn: "t2", change: { kind: "error", will_retry: true, error: { message: "Reconnecting... 2/5" } } }, 6);
  s.tool(
    "t2",
    run(
      "sed -n 15,40p /srv/ingest/src/retry.rs",
      "=== run on laptop (exited 0) ===",
      "    pub fn replay(&self, file: &Path) -> Result<()> {\n        // A failed commit replays the whole file;\n        // every row is read again from its first byte.\n        let start = 0;",
      { duration_ms: 40 },
    ),
    8,
  );
  s.ended("t2", "failed", "stream disconnected before completion: error decoding response body", 30);

  s.heard("t3", user, "继续，retry 那一处改完再跑一下测试。", 120);
  s.push({ event: "delivery_uncertain", delivery: "desktop-notice-1", why: "its app-server stopped while the words were going in" }, 2);
  s.heard("t4", user, "继续，retry 那一处改完再跑一下测试。", 40);
  s.started("t4");
  s.push({ event: "began", turn: "t4", item: { id: "compact-1", kind: "compaction" } });
  s.push({ event: "finished", turn: "t4", item: { id: "compact-1", kind: "compaction" } }, 9);
  s.say(
    "t4",
    "thinking",
    "**Retrying the retry patch**\n\nThe comment spans two lines in the file, which is why the first patch missed. Anchoring on `let start = 0;` alone is unique.",
    2,
  );
  s.tool("t4", {
    name: "apply_patch",
    machine: "laptop",
    input: { input: "*** Begin Patch\n…" },
    output: text("=== apply_patch on laptop (1 file) ===\nM /srv/ingest/src/retry.rs"),
    does: {
      as: "edit",
      changes: [
        {
          path: "/srv/ingest/src/retry.rs",
          how: "updated",
          diff: "@@ -40,5 +40,4 @@ pub fn replay(&self, file: &Path) -> Result<()> {\n         let mut reader = open(file)?;\n-        // A failed commit replays the whole file;\n-        // every row is read again from its first byte.\n-        let start = 0;\n+        // A failed commit replays only the open batch.\n+        let start = self.batch_start(file)?;\n         reader.seek(start)?;\n",
        },
      ],
    },
  });
  s.tool(
    "t4",
    run(
      "cd /srv/ingest && cargo test 2>&1 | tail -3",
      "=== run on laptop (exited 0) ===",
      "test writer::tests::kill_mid_batch_writes_once ... ok\n\ntest result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.84s",
      { duration_ms: 6200 },
    ),
  );
  s.push({
    event: "steps",
    turn: "t4",
    note: null,
    steps: [
      { text: "Move the transaction into Writer::flush", status: "done" },
      { text: "Commit every 5 000 rows and at end of file", status: "done" },
      { text: "Replay only the open batch on retry", status: "done" },
      { text: "Test a kill in the middle of a batch", status: "done" },
    ],
  });
  s.say(
    "t4",
    "message",
    "Done. The writer now commits every 5 000 rows and at end of file, and a retry replays only the rows since the last commit.\n\n- `writer.rs`: the transaction opens lazily in `push` and closes in `flush`.\n- `retry.rs`: `replay` starts at the open batch's first offset.\n- New test `kill_mid_batch_writes_once` kills the writer at row 7 312 and checks every row lands exactly once.\n\nAll 18 tests pass on laptop.",
    3,
  );
  s.push({ event: "context", used: 148_200, window: 272_000 });
  s.ended("t4", "completed");

  return {
    title: "ingest 分批提交",
    agent: "codex",
    resident: "codex-7f3a",
    machine: "laptop",
    model: "gpt-6.1-sol",
    town: "ingest",
    here: "desktop",
    events: [...s.recent(), ...requestEvents({ agent: "codex", count: 38, from: 22_000, to: 148_000, cold: [], compact: 99, compacted: 1, turn: "t1" })],
  };
}

/** A conversation that waits on the user: an approval answered elsewhere,
 *  then one request still waiting, of the kind `?ask=` names (Codex's
 *  command, question, form, link; Claude's claude-permission,
 *  claude-question, claude-plan, claude-form). */
function asking(): Specimen {
  const s = new Script("2026-10-04T15:10:00Z");
  const kind = new URLSearchParams(location.search).get("ask") ?? "command";
  const server = "s-41708";

  s.heard("t1", user, "在 laptop 上把 ingest 的迁移跑一遍，看看新的 batches 表能不能建起来。");
  s.started("t1");
  s.say("t1", "thinking", "**Checking the migration**\n\nThe new migration adds `batches` and moves the commit point; running it needs the local database.", 2);
  s.push({
    event: "changed",
    turn: "t1",
    change: {
      kind: "request",
      id: { server, id: 0 },
      request: "item/commandExecution/requestApproval",
      params: {
        kind: "command",
        threadId: "th-1",
        turnId: "t1",
        itemId: "item-cmd-1",
        startedAtMs: 0,
        environmentId: null,
        command: "cargo sqlx migrate run",
        cwd: "/srv/ingest",
        reason: "needs to reach the local Postgres",
      },
    },
  });
  s.push({ event: "changed", turn: "t1", change: { kind: "resolved", id: { server, id: 0 }, resolution: { threadId: "th-1", requestId: { server, id: 0 } } } }, 20);
  s.tool(
    "t1",
    run("cargo sqlx migrate run", "=== run on laptop (exited 0) ===", "Applied 20261004150000/migrate add batches (31.2ms)", { duration_ms: 900 }),
  );
  s.say("t1", "message", "迁移跑完了：`batches` 表已经建好，旧的 `ingest_files` 没有动。接下来要把离线查询缓存重新生成一遍，`cargo sqlx prepare` 会覆盖 `.sqlx/` 下的文件。", 2);
  s.ended("t1", "completed", null, 1, {
    ms: 72_000,
    facts: {
      tokens: { host: "codex", input_tokens: 108_054, cached_input_tokens: 102_144, output_tokens: 1_175, reasoning_output_tokens: 640, total_tokens: 109_229 },
      limits: { observed_at: "2026-10-04T15:02:10Z", windows: [{ name: "codex", minutes: 10080, used_tenths: 920, resets_at: "2026-10-08T18:00:00Z" }] },
    },
  });

  s.heard("t2", user, "好，接着做。");
  s.started("t2");
  const claudeId = (key: string) => ["claude", "session-9c1e", "1791133200000", key];
  switch (kind) {
    case "claude-permission":
      s.say("t2", "message", "缓存要重新生成，旧的 `.sqlx/` 先清掉。", 2);
      s.push({
        event: "changed",
        turn: "t2",
        change: {
          kind: "request",
          id: claudeId("r-1"),
          request: "classic.PermissionRequest",
          params: {
            session_id: "session-9c1e",
            hook_event_name: "PermissionRequest",
            cwd: "/srv/ingest",
            permission_mode: "default",
            tool_name: "Bash",
            tool_input: { command: "rm -rf .sqlx && cargo sqlx prepare -- --all-targets", description: "Regenerate the offline query cache" },
            permission_suggestions: [
              { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "cargo sqlx prepare:*" }], behavior: "allow", destination: "localSettings" },
              { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "cargo sqlx prepare:*" }], behavior: "allow", destination: "session" },
            ],
          },
        },
      });
      break;
    case "thinking":
      // GPT thinking at length: the row says the section it is in.
      s.say(
        "t2",
        "thinking",
        "**Reading the migration**\n\nThe batches table is in; what remains is the offline query cache.\n\n**Checking whether the batch size survives a replay after a crash**\n\nA smaller batch replays faster after a crash but commits more often.",
        2,
        true,
      );
      break;
    case "goal-question": {
      // A goal is on: Claude's question goes in without stopping its work.
      const asked = [
        {
          question: "批量提交的大小定多少？",
          header: "批量",
          multiSelect: false,
          options: [
            { label: "1000", description: "回放快，提交多" },
            { label: "5000", description: "现在的做法" },
            { label: "20000", description: "提交少，出错时回放多" },
          ],
        },
        {
          question: "这次改动要在哪些机器上验证？",
          header: "机器",
          multiSelect: true,
          options: [
            { label: "laptop", description: "Windows，本地 Postgres" },
            { label: "lab", description: "Linux，跑整套集成测试" },
            { label: "server", description: "线上的副本" },
          ],
        },
      ];
      // A colleague working toward its own goal asked this conversation too,
      // and another handed back what its goal came to.
      const colleague = { machine: "lab", resident: "codex-77a1" };
      const account = { hub: "w-0123456789abcdef", account: "ada" };
      const sidelong = [
        {
          question: "迁移脚本里旧的 ingest_files 要不要顺手删掉？",
          header: "旧表",
          multiSelect: false,
          options: [
            { label: "删掉", description: "回滚要重新导入" },
            { label: "留着", description: "下个版本再删" },
          ],
        },
      ];
      s.heard("t2", { kind: "resident", address: colleague }, "Question from codex-77a1 on lab", 2, {
        kind: "words",
        from: { kind: "resident", address: colleague },
        text: "Question from codex-77a1 on lab",
        goal_question: { id: "q-1", colleague, account, questions: sidelong },
      });
      const done = { machine: "gpu", resident: "codex-3f20" };
      const report = "离线缓存的查询改好了：\n\n- 批次表加了 `replayed_at`，崩溃后从那里接着回放\n- 5000 一批，lab 上整套集成测试过了\n\n没动线上的副本。";
      s.heard("t2", { kind: "resident", address: done }, report, 2, {
        kind: "words",
        from: { kind: "resident", address: done },
        text: report,
        goal_report: {
          id: "r-1",
          completion: "c-1",
          colleague: done,
          account,
          shown: { label: "离线缓存", status: "complete", started_at: "2026-10-23T08:00:00Z", ended_at: "2026-10-23T09:42:00Z", text: report },
        },
      });
      s.say("t2", "message", "我先按 5000 接着做，下面两件你有空再定。", 2);
      s.push({
        event: "finished",
        turn: "t2",
        item: {
          id: JSON.stringify(["fastctx_goal_question", "session-9c1e", "toolu_02"]),
          kind: "message",
          text: asked.map((one) => one.question).join("\n"),
          detail: {
            delivery: "async",
            questions: asked.map((one) => ({ title: one.question, options: one.options.map((option) => option.label), multiSelect: one.multiSelect })),
            fastctx_goal_question: { tool_use_id: "toolu_02", agent_id: null, questions: asked },
          },
        },
      });
      break;
    }
    case "claude-question":
      s.say("t2", "message", "提交之前有两件事想问你。", 2);
      s.push({
        event: "changed",
        turn: "t2",
        change: {
          kind: "request",
          id: claudeId("r-2"),
          request: "AskUserQuestion",
          params: {
            tool: "AskUserQuestion",
            tool_use_id: "toolu_01",
            questions: [
              {
                question: "批量提交的大小定多少？",
                header: "批量",
                multiSelect: false,
                options: [
                  { label: "1000", description: "回放快，提交多" },
                  { label: "5000", description: "现在的做法" },
                  { label: "20000", description: "提交少，出错时回放多" },
                ],
              },
              {
                question: "这次改动要在哪些机器上验证？",
                header: "机器",
                multiSelect: true,
                options: [
                  { label: "laptop", description: "Windows，本地 Postgres" },
                  { label: "lab", description: "Linux，跑整套集成测试" },
                  { label: "server", description: "线上的副本" },
                ],
              },
            ],
          },
        },
      });
      break;
    case "claude-plan":
      s.say("t2", "message", "我把迁移的步骤理了一遍，写成了计划。", 2);
      s.push({
        event: "changed",
        turn: "t2",
        change: {
          kind: "request",
          id: claudeId("r-4"),
          request: "classic.PermissionRequest",
          params: {
            session_id: "session-9c1e",
            hook_event_name: "PermissionRequest",
            cwd: "/srv/ingest",
            permission_mode: "plan",
            tool_name: "ExitPlanMode",
            tool_input: {
              plan: "## 把写入改成批量提交\n\n1. 在 `ingest/src/sink.rs` 里把逐条 `INSERT` 换成 `COPY`，每批 5000 行。\n2. 失败时整批回放，写清是哪一批、第几行。\n3. 重新生成 `.sqlx/` 的离线缓存。\n4. 在 lab 上跑整套集成测试，对比改前改后的吞吐。\n\n不动的：读取的那一半、表结构。",
              planFilePath: "/home/dev/.claude/plans/batch-commit.md",
            },
            permission_suggestions: [{ type: "setMode", mode: "acceptEdits", destination: "session" }],
          },
        },
      });
      break;
    case "claude-form":
      s.say("t2", "message", "我把这次迁移记成一个 issue。", 2);
      s.push({
        event: "changed",
        turn: "t2",
        change: {
          kind: "request",
          id: claudeId("r-3"),
          request: "Elicitation",
          params: {
            session_id: "session-9c1e",
            hook_event_name: "Elicitation",
            mcp_server_name: "linear",
            mode: "form",
            message: "新建 issue 之前，请确认这几样：",
            requested_schema: {
              type: "object",
              required: ["title"],
              properties: {
                title: { type: "string", title: "标题", default: "ingest: commit in batches of 5000" },
                priority: { type: "string", title: "优先级", enum: ["urgent", "high", "normal", "low"], enumNames: ["紧急", "高", "普通", "低"], default: "normal" },
              },
            },
          },
        },
      });
      break;
    case "question":
      s.say("t2", "message", "生成缓存之前要连上测试库，有两件事得问你。", 2);
      s.push({
        event: "changed",
        turn: "t2",
        change: {
          kind: "request",
          id: { server, id: 1 },
          request: "item/tool/requestUserInput",
          params: {
            threadId: "th-1",
            turnId: "t2",
            itemId: "item-ask-1",
            isBlocking: true,
            autoResolutionMs: null,
            questions: [
              {
                id: "where",
                header: "连接串",
                question: "测试库的连接串放在哪里？",
                isOther: true,
                isSecret: false,
                options: [
                  { label: ".env", description: "仓库根目录下，已在 .gitignore 里" },
                  { label: "环境变量", description: "DATABASE_URL，只在这台机器上" },
                ],
              },
              { id: "password", header: "密码", question: "测试库 ingest_test 的密码是什么？", isOther: false, isSecret: true, options: null },
            ],
          },
        },
      });
      break;
    case "form":
      s.say("t2", "message", "缓存重新生成了。我把这次迁移记成一个 issue。", 2);
      s.push({
        event: "changed",
        turn: "t2",
        change: {
          kind: "request",
          id: { server, id: 1 },
          request: "mcpServer/elicitation/request",
          params: {
            threadId: "th-1",
            turnId: "t2",
            serverName: "linear",
            mode: "form",
            _meta: null,
            message: "新建 issue 之前，请确认这几样：",
            requestedSchema: {
              type: "object",
              required: ["title", "priority"],
              properties: {
                title: { type: "string", title: "标题", default: "ingest: commit in batches of 5000" },
                priority: {
                  type: "string",
                  title: "优先级",
                  oneOf: [
                    { const: "urgent", title: "紧急" },
                    { const: "high", title: "高" },
                    { const: "normal", title: "普通" },
                    { const: "low", title: "低" },
                  ],
                  default: "normal",
                },
                labels: { type: "array", title: "标签", items: { type: "string", enum: ["ingest", "database", "migration"] }, default: ["ingest"] },
                due: { type: "string", title: "截止日期", format: "date" },
                notify: { type: "boolean", title: "通知团队", description: "在 #ingest 频道里说一声", default: true },
              },
            },
          },
        },
      });
      break;
    case "link":
      s.say("t2", "message", "要把分支推上去，GitHub 那边需要你授权一次。", 2);
      s.push({
        event: "changed",
        turn: "t2",
        change: {
          kind: "request",
          id: { server, id: 1 },
          request: "mcpServer/elicitation/request",
          params: {
            threadId: "th-1",
            turnId: "t2",
            serverName: "github",
            mode: "url",
            _meta: null,
            message: "在浏览器里登录 GitHub，并授权 fastctx-ingest 读写这个仓库。",
            url: "https://github.com/login/device",
            elicitationId: "el-1",
          },
        },
      });
      break;
    default:
      s.push({
        event: "changed",
        turn: "t2",
        change: {
          kind: "request",
          id: { server, id: 1 },
          request: "item/commandExecution/requestApproval",
          params: {
            kind: "command",
            threadId: "th-1",
            turnId: "t2",
            itemId: "item-cmd-2",
            startedAtMs: 0,
            environmentId: null,
            command: "rm -rf .sqlx && cargo sqlx prepare -- --all-targets",
            cwd: "/srv/ingest",
            reason: "regenerates the offline query cache; the old files under .sqlx are removed first",
            proposedExecpolicyAmendment: ["cargo", "sqlx", "prepare"],
            availableDecisions: [
              "accept",
              "acceptForSession",
              { acceptWithExecpolicyAmendment: { execpolicy_amendment: ["cargo", "sqlx", "prepare"] } },
              "decline",
              "cancel",
            ],
          },
        },
      }, 3);
  }
  s.push({ event: "context", used: 61_400, window: 272_000 });

  const claude = kind.startsWith("claude-");
  return {
    title: "ingest 迁移",
    agent: claude ? "claude" : "codex",
    resident: claude ? "claude-2b91" : "codex-a51c",
    machine: "laptop",
    model: claude ? "claude-opus-5-5" : "gpt-6.1-sol",
    town: "ingest",
    here: "desktop",
    events: s.recent(),
  };
}

/** A Claude conversation's commands as the World tells them: a compaction
 *  done, one whose outcome nobody can tell, and one that failed. */
function controls(): Specimen {
  const s = new Script("2026-10-24T13:40:00Z");
  s.heard("t1", user, "把 ingest 的迁移说明整理一下，写进 MIGRATING.md。");
  s.started("t1");
  s.say("t1", "message", "写好了：`MIGRATING.md` 里按顺序列了三步，回滚的办法放在最后。", 4);
  s.ended("t1", "completed");
  const row = (id: number, name: string, state: "completed" | "uncertain" | "failed", error: string | null) =>
    s.push({
      event: "changed",
      turn: null,
      change: {
        kind: "metadata",
        metadata: {
          fastctx_control: {
            id: `control-1792843104777-${id}-0`,
            control: { do: "command", name, args: "" },
            at: "2026-10-24T13:41:00Z",
            state,
            result: state === "completed" ? { kind: "done", said: null } : null,
            error,
          },
        } as unknown as JsonValue,
      },
    }, 3);
  row(1, "compact", "completed", null);
  row(2, "compact", "uncertain", "the Claude activation ended without a receipt; the control's outcome is unknown and it will not be repeated");
  row(3, "init", "failed", "AGENTS.md could not be written: the specimen writes nothing");
  return { title: "迁移说明", agent: "claude", resident: "claude-c0ntrols", machine: "laptop", model: "claude-opus-5-5", town: "ingest", here: "desktop", events: s.recent() };
}

export const specimens: Record<string, () => Specimen> = { claude, codex, asking, steps, trips, tour, deeds, controls, voice };
