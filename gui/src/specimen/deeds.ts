// A specimen conversation whose runs are drawn as what they did
// (`conversation/deeds`): a search through the code, a file looked at, the
// tests failing and then passing after a change, the checks clean, a
// commit pushed; and a command nothing of which is recognised, shown in its
// own words.

import { run, Script } from "./script";
import { cut, hunk, noted, printed, put, ran, same, wrote } from "./run-facts";
import type { Speaker } from "../world/events";
import type { Specimen } from "./sessions";
import { longWork } from "./deeds-long";
import { worldWork } from "./deeds-world";

const user: Speaker = { kind: "user", machine: "desktop" };

export function deeds(): Specimen {
  const s = new Script(new Date(Date.now() - 22 * 60_000).toISOString());
  s.heard("t1", user, "kill_mid_batch_writes_once 这个测试一直挂，查一下，修好了提交推上去。");
  s.started("t1");
  s.say("t1", "thinking", "**Finding where the batch commits**\n\nThe test kills the writer mid-batch and counts rows; a row written twice means the replay starts too early.", 2);

  s.tool(
    "t1",
    run(
      "rg -n 'fn flush|begin_tx|first_offset' /srv/ingest/src",
      "=== run on laptop (exited 0) ===",
      [
        "/srv/ingest/src/writer.rs:41:        let tx = self.tx.get_or_insert_with(|| self.db.begin_tx());",
        "/srv/ingest/src/writer.rs:77:    pub fn flush(&mut self) -> Result<()> {",
        "/srv/ingest/src/retry.rs:23:        let start = batch.first_offset;",
        "/srv/ingest/src/retry.rs:58:    fn first_offset(&self) -> u64 {",
      ].join("\n"),
      { duration_ms: 140 },
    ),
  );

  s.tool(
    "t1",
    run(
      "fastctx view -- /srv/ingest/src/retry.rs",
      "=== run on laptop (exited 0) ===",
      [
        "=== view (1 file: 1 shown) ===",
        "=== laptop:/srv/ingest/src/retry.rs (lines 1-12 and 52-64 of 64) ===",
        "1\tuse crate::writer::Batch;",
        "2\tuse std::path::Path;",
        "3\t",
        "4\t/// Replays what a crash left unwritten.",
        "5\tpub struct Retry {",
        "6\t    pending: Vec<Batch>,",
        "7\t}",
        "8\t",
        "9\timpl Retry {",
        "10\t    pub fn replay(&self, file: &Path) -> Result<()> {",
        "11\t        let batch = self.pending.last().ok_or(Error::Nothing)?;",
        "12\t        let start = batch.first_offset;",
        "=== lines 13-51 not shown ===",
        "52\t    }",
        "53\t",
        "54\t    /// Where the open batch began: one past the last committed row.",
        "55\t    fn first_offset(&self) -> u64 {",
        "56\t        self.committed",
        "57\t    }",
        "58\t}",
      ].join("\n"),
      { duration_ms: 80 },
    ),
  );

  const failing = [
    "   Compiling ingest v0.3.0 (/srv/ingest)",
    "warning: unused variable: `row`",
    "  --> src/writer.rs:44:13",
    "   |",
    "44 |         let row = self.rows.len();",
    "   |             ^^^ help: if this is intentional, prefix it with an underscore: `_row`",
    "   |",
    "   = note: `#[warn(unused_variables)]` on by default",
    "",
    "warning: `ingest` (lib test) generated 1 warning",
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.42s",
    "     Running unittests src/lib.rs (target/debug/deps/ingest-5b1f0e2a9c4d7781)",
    "",
    "running 18 tests",
    "test retry::tests::replays_open_batch ... ok",
    "test retry::tests::nothing_pending ... ok",
    "test writer::tests::commits_every_batch ... ok",
    "test writer::tests::kill_mid_batch_writes_once ... FAILED",
    "test writer::tests::slow_disk ... ignored",
    "",
    "failures:",
    "",
    "---- writer::tests::kill_mid_batch_writes_once stdout ----",
    "thread 'writer::tests::kill_mid_batch_writes_once' panicked at src/writer.rs:131:9:",
    "assertion `left == right` failed: row 7 312 was written twice",
    "  left: 7313",
    " right: 7312",
    "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
    "",
    "failures:",
    "    writer::tests::kill_mid_batch_writes_once",
    "",
    "test result: FAILED. 16 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.84s",
    "",
    "error: test failed, to rerun pass `--lib`",
  ].join("\n");
  s.tool("t1", run("cd /srv/ingest && cargo test 2>&1 | tail -40", "=== run on laptop (exited 101) ===", failing, { duration_ms: 8_200 }));

  s.say("t1", "thinking", "**The replay starts one row early**\n\n`first_offset` returns the last committed row itself, not the one after it.", 2);

  // The change and both checks in one script, wrapped in an `on` to the
  // machine it already runs on, as GPT writes them.
  const operation = "w-7c21";
  const write = wrote(operation, 1, "laptop", "/srv/ingest/src/retry.rs", "modified", [
    hunk(54, 54),
    same(54, 54, "    /// Where the open batch began: one past the last committed row."),
    same(55, 55, "    fn first_offset(&self) -> u64 {"),
    cut(56, "        self.committed"),
    put(56, "        self.committed + 1"),
    same(57, 57, "    }"),
  ]);
  const fixing = [
    "    Checking ingest v0.3.0 (/srv/ingest)",
    "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.62s",
    "   Compiling ingest v0.3.0 (/srv/ingest)",
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.10s",
    "     Running unittests src/lib.rs (target/debug/deps/ingest-5b1f0e2a9c4d7781)",
    "",
    "running 18 tests",
    "test writer::tests::kill_mid_batch_writes_once ... ok",
    "test writer::tests::slow_disk ... ignored",
    "",
    "test result: ok. 17 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.81s",
  ].join("\n");
  const script = [
    "on laptop <<'RUN'",
    "set -e",
    "cd /srv/ingest",
    "edit src/retry.rs <<'EOF'",
    "<<<<<<< SEARCH",
    "        self.committed",
    "=======",
    "        self.committed + 1",
    ">>>>>>> REPLACE",
    "EOF",
    "cargo clippy --all-targets -- -D warnings",
    "cargo test",
    "RUN",
  ].join("\n");
  s.tool("t1", {
    ...run(script, "=== run on laptop (exited 0) ===", fixing, { duration_ms: 9_400 }),
    run: ran("laptop", "j-7c21aa", { kind: "ended", status: { code: 0 } }, [noted("edit", operation, 1, "=== edit laptop:/srv/ingest/src/retry.rs (+1 −1) ===\n"), printed(2, `${fixing}\n`)], [write]),
  });

  s.tool(
    "t1",
    run(
      'git status --short && git commit -am "Replay a crashed batch from the row after the last commit" && git push origin rebuild',
      "=== run on laptop (exited 0) ===",
      [
        " M src/retry.rs",
        " M src/writer.rs",
        "[rebuild 7e8d3d8] Replay a crashed batch from the row after the last commit",
        " 2 files changed, 2 insertions(+), 2 deletions(-)",
        "To github.com:mark50/ingest.git",
        "   3e4b8bb..7e8d3d8  rebuild -> rebuild",
      ].join("\n"),
      { duration_ms: 2_300 },
    ),
  );

  s.tool(
    "t1",
    run(
      "python scripts/soak.py --rounds 3 --kill-at 7312",
      "=== run on laptop (exited 0) ===",
      ["round 1: 50 000 rows, killed at 7 312, replayed 1 batch, 0 doubled", "round 2: 50 000 rows, killed at 7 312, replayed 1 batch, 0 doubled", "round 3: 50 000 rows, killed at 7 312, replayed 1 batch, 0 doubled"].join("\n"),
      { duration_ms: 31_000 },
    ),
  );

  s.say(
    "t1",
    "message",
    "修好了。重放从上次提交的那一行开始，于是那一行被写了两次；`first_offset` 改成返回它的下一行。clippy 干净，18 个测试里 17 个过、1 个本来就跳过；连跑三轮掐断也没有重复的行。已提交 `7e8d3d8` 并推到 `rebuild`。",
    2,
  );
  s.ended("t1", "completed", null, 1);
  worldWork(s, user);
  longWork(s, user);
  return { title: "修重放", agent: "codex", resident: "deeds-1", machine: "laptop", model: "gpt-6.1-sol", town: "mark50", here: "desktop", events: s.recent() };
}
