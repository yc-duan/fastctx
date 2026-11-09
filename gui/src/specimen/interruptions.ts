// Notices between tools, including a call that completes after a notice.
import type { Specimen } from "./sessions";
import { run, Script } from "./script";

export function interruptions(): Specimen {
  const s = new Script("2026-10-04T13:02:00Z");
  const turn = "t1";
  s.heard(turn, { kind: "user", machine: "desktop" }, "检查积分溢出的原因，等复现结果回来后再跑测试。");
  s.started(turn);
  s.tool(turn, run("fastctx view src/ctrl/fixed.rs", "=== run on laptop (exited 0) ===", "The integral uses Q32."));
  const long = run("cargo test -p ctrl", "", "");
  const call = s.tool(turn, long, 2, true);
  const started = s.events.at(-1)!;
  s.push({ event: "finished", turn, item: { id: "compact", kind: "compaction" } }, 30);
  s.tool(turn, run("fastctx view src/ctrl/q16.rs", "=== run on laptop (exited 0) ===", "The output narrows to Q16."));
  const colleague = { machine: "laptop", resident: "codex-repro" };
  s.heard(turn, { kind: "resident", address: colleague }, "复现已完成。", 2, {
    kind: "answer", colleague, label: "GPT-6.1-Sol", turn: "repro",
    end: { how: "completed", reason: null, duration_ms: 30_000, last_message: "积分没有溢出，收窄为 Q16 时发生回绕。" },
  });
  s.tool(turn, run("cargo test -p ctrl overflow_repro", "=== run on laptop (exited 0) ===", "1 passed"));
  s.push({ event: "finished", turn, item: { id: "agent-return", kind: "agent_activity", agent: "review", path: "/review", activity: "completed" } });
  s.tool(turn, run("git diff --check", "=== run on laptop (exited 0) ===", ""));
  if (started.event === "began" && started.item.kind === "tool") {
    s.push({ event: "finished", turn, item: { ...started.item, id: call, status: "done", output: [{ type: "text", text: "42 passed" }] } });
  }
  s.say(turn, "message", "复现和整套测试均已通过。");
  s.ended(turn, "completed");
  return { title: "积分溢出复现", agent: "codex", resident: "codex-order", machine: "laptop", model: "gpt-6.1-sol", town: "ctrl", here: "desktop", events: s.recent() };
}
