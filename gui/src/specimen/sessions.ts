// Two sessions written the way the World sends them, for looking at the
// interface without a world behind it: every kind of thing a conversation
// shows, from both hosts, in the shapes the node really produces.

import type { Content, Does, Item, SessionEvent, Speaker, Tool } from "../world/events";

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

class Script {
  events: SessionEvent[] = [];
  private clock: number;
  private serial = 0;

  constructor(start: string) {
    this.clock = Date.parse(start);
  }

  private at(seconds: number): string {
    this.clock += seconds * 1000;
    return new Date(this.clock).toISOString();
  }

  private id(): string {
    this.serial += 1;
    return `item-${this.serial}`;
  }

  heard(turn: string | null, from: Speaker | null, text: string, after = 4): void {
    this.events.push({ at: this.at(after), event: "heard", turn, from, text });
  }

  started(turn: string): void {
    this.events.push({ at: this.at(1), event: "turn_started", turn });
  }

  say(turn: string, kind: "message" | "thinking" | "plan", text: string, after = 3, writing = false): void {
    const item = { id: this.id(), kind, text } as Item;
    this.events.push({ at: this.at(after), event: "began", turn, item: { ...item, text: "" } as Item });
    if (writing) {
      this.events.push({ at: this.at(1), event: "grew", turn, item: item.id, part: "text", text });
      return;
    }
    this.events.push({ at: this.at(1), event: "finished", turn, item });
  }

  tool(turn: string, tool: Partial<Tool> & { name: string; does: Does }, after = 2, running = false): void {
    const full: Tool = {
      server: "fastctx",
      machine: null,
      input: {},
      output: [],
      status: running ? "running" : "done",
      duration_ms: null,
      ...tool,
    };
    const id = this.id();
    const began = this.at(after);
    this.events.push({
      at: began,
      event: "began",
      turn,
      item: { id, started_at: began, kind: "tool", ...full, status: "running", output: [] },
    });
    if (running) return;
    this.events.push({ at: this.at(Math.max(1, (full.duration_ms ?? 800) / 1000)), event: "finished", turn, item: { id, kind: "tool", ...full } });
  }

  ended(turn: string, how: string, reason: string | null = null, after = 1): void {
    this.events.push({
      at: this.at(after),
      event: "turn_ended",
      turn,
      end: { how, reason, duration_ms: null, last_message: null },
    });
  }

  push(event: import("../world/events").Happened, after = 1): void {
    this.events.push({ at: this.at(after), ...event } as SessionEvent);
  }
}

const text = (value: string): Content[] => [{ type: "text", text: value }];

const user: Speaker = { kind: "user", machine: "desktop" };

function run(command: string, head: string, body: string, extra: Partial<Tool> = {}, node?: string): Partial<Tool> & { name: string; does: Does } {
  const exit = /exited (\d+)/.exec(head);
  const job = /; (j-[0-9a-z]+)\)/.exec(head);
  return {
    name: "run",
    machine: node ?? "laptop",
    input: node ? { command, node } : { command },
    output: text(`${head}\n${body}`),
    does: { as: "command", command, cwd: null, exit: exit ? Number(exit[1]) : null, job: job ? job[1] : null },
    ...extra,
  };
}

function edit(path: string, before: string, after: string, extra: Partial<Tool> = {}): Partial<Tool> & { name: string; does: Does } {
  const lines = (mark: string, value: string) =>
    value
      .split("\n")
      .map((line) => `${mark}${line}`)
      .join("\n");
  return {
    name: "edit",
    machine: "laptop",
    input: { file_path: path, old_string: before, new_string: after },
    output: text(`=== edit on laptop (${path}: 1 replacement) ===`),
    does: { as: "edit", changes: [{ path, how: "updated", diff: `${lines("-", before)}\n${lines("+", after)}\n` }] },
    ...extra,
  };
}

function claude(): Specimen {
  const s = new Script("2026-10-04T13:02:00Z");

  s.heard("t1", user, "lab 上 ctrl 的测试一直挂在 step_response，帮我看看是怎么回事，顺手修掉。修完在 lab 上把整套测试跑一遍。");
  s.started("t1");
  s.say("t1", "thinking", "", 2);
  s.tool(
    "t1",
    run(
      "rg -n 'fn step' /home/lenovo/mark50/src/ctrl",
      "=== run on laptop (exited 0) ===",
      "/home/lenovo/mark50/src/ctrl/pid.rs:41:    pub fn step(&mut self, error: f32, dt: f32) -> f32 {\n/home/lenovo/mark50/src/ctrl/fixed.rs:58:    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {",
      { duration_ms: 84 },
    ),
  );
  s.tool(
    "t1",
    run(
      "fastctx view -- /home/lenovo/mark50/src/ctrl/fixed.rs /home/lenovo/mark50/src/ctrl/pid.rs",
      "=== run on laptop (exited 0) ===",
      "=== view (2 files: 2 shown) ===\n=== laptop:/home/lenovo/mark50/src/ctrl/fixed.rs (lines 1-112 of 112) ===\n1\t//! Fixed-point PID for the joint controllers.\n2\t\n3\tuse crate::q16::Q16;\n…",
      { duration_ms: 120 },
    ),
  );
  s.tool(
    "t1",
    run(
      "cd /srv/mark50 && cargo test -p ctrl step_response 2>&1 | tail -12",
      "=== run on lab (exited 101) ===",
      [
        "running 1 test",
        "test fixed::tests::step_response ... FAILED",
        "",
        "failures:",
        "",
        "---- fixed::tests::step_response stdout ----",
        "thread 'fixed::tests::step_response' panicked at ctrl/src/fixed.rs:131:9:",
        "assertion `left == right` failed: settled at 0.9612, wanted 1.0000 ± 0.002",
        "  left: 0.9612",
        " right: 1.0000",
        "",
        "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 41 filtered out",
      ].join("\n"),
      { duration_ms: 9400 },
      "lab",
    ),
  );
  s.say("t1", "thinking", "", 3);
  s.tool(
    "t1",
    edit(
      "/home/lenovo/mark50/src/ctrl/fixed.rs",
      [
        "    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {",
        "        let p = self.kp * error;",
        "        self.integral += error * dt;",
        "        let i = self.ki * self.integral;",
        "        let d = self.kd * (error - self.last) / dt;",
      ].join("\n"),
      [
        "    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {",
        "        let p = self.kp * error;",
        "        // Accumulate in Q32: a slow error is otherwise lost to rounding.",
        "        self.integral = self.integral.saturating_add(error.widen() * dt.widen());",
        "        let i = (self.ki.widen() * self.integral).narrow();",
        "        let d = self.kd * (error - self.last) / dt;",
      ].join("\n"),
    ),
  );
  s.tool(
    "t1",
    edit(
      "/home/lenovo/mark50/src/ctrl/fixed.rs",
      "    integral: Q16,",
      "    integral: Q32,",
    ),
    1,
  );
  s.tool(
    "t1",
    run(
      "cd /srv/mark50 && cargo test --workspace",
      "=== run on lab (still running after 15s; j-k3x9a2) ===",
      "   Compiling ctrl v0.4.1 (/srv/mark50/ctrl)\n   Compiling joints v0.4.1 (/srv/mark50/joints)\n=== output continues in lab /home/lenovo/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-k3x9a2.log; you'll be told when it ends ===",
      { duration_ms: 15000 },
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
  s.ended("t1", "completed");

  s.heard(
    "t2",
    { kind: "world" },
    "=== j-k3x9a2 on lab ended (exited 0) at 2026-10-04 13:09:12 UTC; its output is in /home/lenovo/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-k3x9a2.log ===",
    190,
  );
  s.started("t2");
  s.say("t2", "thinking", "", 2);
  s.tool(
    "t2",
    run(
      "tail -4 /home/lenovo/.fastctx/world/towns/mark50/residents/claude-2b1c/out/j-k3x9a2.log",
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
        "We moved the PID integral in /home/lenovo/mark50/src/ctrl/fixed.rs to Q32 with saturating_add. Make it fail: find inputs where the integral or the narrowed output wraps instead of saturating, with a test that reproduces it.",
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

  s.heard(
    "t3",
    { kind: "resident", address: { machine: "laptop", resident: "codex-91c0" } },
    "=== colleague codex-91c0 on laptop (turn completed, 14m) ===\nFound one. With `ki = 0x7FFF_0000` and a constant error of 32767 at `dt = 1`, the integral saturates after 65 538 steps as meant, but `narrow()` then **wraps** instead of saturating: the output jumps from 32767 to −32768.\n\nRepro: `cargo test -p ctrl overflow_repro` on branch `colleague/q16-edge` (laptop, `/home/lenovo/wt/q16-edge`).\n=== what it did: fastctx w codex-91c0 ===",
    840,
  );
  s.started("t3");
  s.say("t3", "thinking", "", 2);
  s.tool(
    "t3",
    edit(
      "/home/lenovo/mark50/src/ctrl/q16.rs",
      "    pub fn narrow(self) -> Q16 {\n        Q16((self.0 >> 16) as i32)\n    }",
      "    pub fn narrow(self) -> Q16 {\n        Q16((self.0 >> 16).clamp(i32::MIN as i64, i32::MAX as i64) as i32)\n    }",
    ),
  );
  s.tool(
    "t3",
    run(
      "cd /srv/mark50 && git fetch laptop colleague/q16-edge && cargo test -p ctrl overflow_repro",
      "",
      "",
      {},
      "lab",
    ),
    2,
    true,
  );

  return {
    title: "ctrl 的 step_response",
    agent: "claude",
    resident: "claude-2b1c",
    machine: "laptop",
    model: "Opus 5.5",
    town: "mark50",
    here: "desktop",
    events: s.events,
  };
}

function codex(): Specimen {
  const s = new Script("2026-10-04T08:40:00Z");

  s.heard("t1", user, "ingest 服务现在每个文件提交一次，大文件会把事务拖得很长。改成每 5000 条一批提交，先给我个计划。");
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
  s.push({ event: "delivery_uncertain", why: "its app-server stopped while the words were going in" }, 2);
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
          diff: "@@ pub fn replay(&self, file: &Path) -> Result<()> {\n-        // A failed commit replays the whole file;\n-        // every row is read again from its first byte.\n-        let start = 0;\n+        // A failed commit replays only the open batch.\n+        let start = self.batch_start(file)?;\n",
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
    events: s.events,
  };
}

export const specimens: Record<string, () => Specimen> = { claude, codex };
