// A long stretch of work in the deeds specimen (`deeds.ts`): one turn that
// reads its way through a crate, then changes it round after round, the
// tests failing and passing between, a command or two of its own, and a
// commit at the end; and a turn still running a long command. What a
// stretch of a hundred steps looks like, folded and open.

import { run, type Script } from "./script";
import { cut, hunk, noted, put, ran, same, wrote } from "./run-facts";
import type { Speaker } from "../world/events";

const FILES = ["lib.rs", "writer.rs", "retry.rs", "batch.rs", "queue.rs", "sink.rs", "config.rs", "metrics.rs", "schema.rs", "codec.rs", "spool.rs", "clock.rs", "error.rs", "pool.rs"];

function viewed(path: string, lines: number): string {
  const rows = Array.from({ length: Math.min(8, lines) }, (_, index) => `${index + 1}\t// ${path} line ${index + 1}`);
  return ["=== view (1 file: 1 shown) ===", `=== laptop:/srv/ingest/src/${path} (lines 1-${rows.length} of ${lines}) ===`, ...rows].join("\n");
}

function tested(passed: number, failed: string[]): string {
  return [
    "   Compiling ingest v0.4.0 (/srv/ingest)",
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.80s",
    "     Running unittests src/lib.rs (target/debug/deps/ingest-5b1f0e2a9c4d7781)",
    "",
    `running ${passed + failed.length} tests`,
    ...failed.map((name) => `test ${name} ... FAILED`),
    "",
    ...(failed.length ? ["failures:", "", ...failed.flatMap((name) => [`---- ${name} stdout ----`, `thread '${name}' panicked at src/spool.rs:88:5:`, "assertion failed: spooled.is_empty()", ""]), "failures:", ...failed.map((name) => `    ${name}`), ""] : []),
    `test result: ${failed.length ? "FAILED" : "ok"}. ${passed} passed; ${failed.length} failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.20s`,
  ].join("\n");
}

export function longWork(s: Script, user: Speaker): void {
  s.heard("t2", user, "把写入改成异步的：攒满一批或者过了 200 毫秒就落盘，崩了也不能丢。改完把测试都跑通再提交。");
  s.started("t2");
  s.say("t2", "thinking", "**Reading the crate before changing it**", 2);
  for (const [index, file] of FILES.entries()) s.tool("t2", run(`fastctx view -- /srv/ingest/src/${file}`, "=== run on laptop (exited 0) ===", viewed(file, 60 + index * 17), { duration_ms: 60 }), 1);
  s.tool("t2", run("rg -n 'fn flush|spawn|Instant::now' /srv/ingest/src", "=== run on laptop (exited 0) ===", ["/srv/ingest/src/writer.rs:77:    pub fn flush(&mut self) -> Result<()> {", "/srv/ingest/src/clock.rs:12:    let now = Instant::now();"].join("\n"), { duration_ms: 90 }), 1);
  s.say("t2", "thinking", "**A spool between the writer and the disk**", 2);
  const rounds: [string[], string[]][] = [
    [["spool.rs", "writer.rs", "lib.rs"], ["spool::tests::drains_on_timer", "spool::tests::survives_kill"]],
    [["spool.rs", "clock.rs"], ["spool::tests::survives_kill"]],
    [["spool.rs", "writer.rs", "batch.rs", "error.rs"], []],
  ];
  rounds.forEach(([changed, failing], round) => {
    for (const [index, file] of changed.entries()) {
      const operation = `w-${round}${index}`;
      const write = wrote(operation, 1, "laptop", `/srv/ingest/src/${file}`, round === 0 && file === "spool.rs" ? "added" : "modified", [
        hunk(10 + index * 7, 10 + index * 7),
        same(10, 10, "    let spool = Spool::new(config);"),
        cut(11, "    self.flush()?;"),
        put(11, "    spool.push(row)?;"),
        put(12, "    spool.tick(now)?;"),
      ]);
      s.tool(
        "t2",
        {
          ...run(`edit /srv/ingest/src/${file} <<'EOF'\n…\nEOF`, "=== run on laptop (exited 0) ===", "", { duration_ms: 120 }),
          run: ran("laptop", `j-e${round}${index}`, { kind: "ended", status: { code: 0 } }, [noted("edit", operation, 1, `=== edit laptop:/srv/ingest/src/${file} (+2 −1) ===\n`)], [write]),
        },
        1,
      );
    }
    s.tool("t2", run("cd /srv/ingest && cargo test 2>&1 | tail -60", `=== run on laptop (exited ${failing.length ? 101 : 0}) ===`, tested(24 + round, failing), { duration_ms: 7_400 }), 1);
    if (failing.length) s.say("t2", "thinking", `**${failing.length} still failing**`, 2);
  });
  s.tool("t2", run("mkdir -p /srv/ingest/spool && ls -la /srv/ingest/spool", "=== run on laptop (exited 0) ===", "total 0", { duration_ms: 40 }), 1);
  s.tool("t2", run("python scripts/soak.py --rounds 5 --kill-at random", "=== run on laptop (exited 0) ===", "5 rounds, 0 lost, 0 doubled", { duration_ms: 42_000 }), 1);
  s.tool("t2", run("cargo clippy --all-targets -- -D warnings", "=== run on laptop (exited 0) ===", "    Checking ingest v0.4.0 (/srv/ingest)\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.10s", { duration_ms: 2_100 }), 1);
  s.tool(
    "t2",
    run('git commit -am "Write through a spool: a batch or 200 ms, whichever comes first" && git push origin rebuild', "=== run on laptop (exited 0) ===", ["[rebuild 91c0a2f] Write through a spool: a batch or 200 ms, whichever comes first", " 6 files changed, 148 insertions(+), 31 deletions(-)", "To github.com:mark50/ingest.git", "   7e8d3d8..91c0a2f  rebuild -> rebuild"].join("\n"), { duration_ms: 2_000 }),
    1,
  );
  s.say("t2", "message", "改好了：写入先进 spool，攒满一批或过 200 毫秒落盘；崩了从 spool 续写。27 个测试全过，五轮随机掐断没丢也没重。已提交 `91c0a2f` 并推到 `rebuild`。", 2);
  s.ended("t2", "completed", null, 1);

  // Still going: a long benchmark, the mark at the end of its strip
  // breathing.
  s.heard("t3", user, "顺手跑一下基准，看看吞吐有没有掉。");
  s.started("t3");
  s.tool("t3", run("fastctx view -- /srv/ingest/benches/throughput.rs", "=== run on laptop (exited 0) ===", viewed("../benches/throughput.rs", 40), { duration_ms: 60 }), 1);
  // A log looked at again and again while a job goes on: one row, "×4".
  for (let index = 0; index < 4; index += 1) s.tool("t3", run("tail -n 3 /srv/ingest/target/bench.log", "=== run on laptop (exited 0) ===", `throughput: ${118 + index} k rows/s`, { duration_ms: 40 }), 1);
  s.tool("t3", { ...run("cargo bench --bench throughput", "", ""), status: "running" }, 1, true);
}

