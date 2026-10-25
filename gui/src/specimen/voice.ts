// A night of work by a conversation the World started (`world-voice.md`):
// the user hands it a fix and a watch before bed; a job's end and a
// sentinel wake it in the night; it works, and says to the user only what
// they should know, once urgently with the picture that shows why. In the
// morning the user asks one thing and it answers in plain words, as an
// agent that forgets the tool does, which the view takes as its answer; so
// it does for the turn a command the user gives begins.

import { run, Script, text } from "./script";
import type { Specimen } from "./sessions";
import type { Speaker, Tool } from "../world/events";
import type { JsonValue } from "../world/wire";

const user: Speaker = { kind: "user", machine: "desktop" };

/** A `message_user` call, as the World's tools show it in either host. */
function say(message: string, status: "reply" | "info" | "urgent", attachments: { machine: string; path: string }[] = []): Partial<Tool> & { name: string; does: Tool["does"] } {
  return {
    name: "message_user",
    machine: null,
    input: { message, status, ...(attachments.length ? { attachments } : {}) },
    output: text("=== told the user ==="),
    does: { as: "call" },
    duration_ms: 40,
  };
}

export function voice(): Specimen {
  const s = new Script("2026-10-04T14:10:00Z");

  s.heard("t1", user, "ctrl 的 step_response 修好了，今晚在 lab 上把整套回归跑完，跑完告诉我。训练那边也帮我盯着，loss 发散了就处理。", 4);
  s.started("t1");
  s.say("t1", "thinking", "", 2);
  s.tool("t1", run("cd /srv/mark50 && git log --oneline -3", "=== run on lab (exited 0) ===", "b81e3f2 Widen the integral to Q32\n2d90a1c Record step-response vectors\n77c0e45 Bound the derivative kick", { duration_ms: 160 }, "lab"));
  s.tool(
    "t1",
    run(
      "cd /srv/mark50 && cargo test --workspace --release 2>&1",
      "=== run on lab (still running; j-r4g7e2) ===",
      "   Compiling ctrl v0.4.1 (/srv/mark50/ctrl)\n=== j-r4g7e2 goes on in the background; all of its output is in /home/me/.fastctx/world/towns/mark50/residents/claude-v01c/out/j-r4g7e2.log ===",
      { duration_ms: 15_000 },
      "lab",
    ),
  );
  s.tool("t1", run("fastctx sense 'the training loss on gpu diverges or stalls' --tell 'look at run3'", "=== run on laptop (exited 0) ===", "=== sentinel s-7f2c on server watches it ===", { duration_ms: 900 }));
  s.tool("t1", say("好。整套回归已经在 lab 上跑起来，大约四十分钟。训练那边挂了一个哨兵盯着 loss，发散或停住了它会叫醒我。有结果我再告诉你。", "reply"));
  s.say("t1", "message", "回归 j-r4g7e2 在 lab 上跑着，哨兵 s-7f2c 在 gpu 上盯着 run3。它们叫醒我之前没有别的事。");
  s.ended("t1", "completed");

  s.heard(
    "t2",
    { kind: "world" },
    "=== j-r4g7e2 on lab ended (exited 0) at 2026-10-04 14:52:30 UTC; its output is in /home/me/.fastctx/world/towns/mark50/residents/claude-v01c/out/j-r4g7e2.log ===",
    2400,
    {
      kind: "job_ended",
      job: "j-r4g7e2",
      machine: "lab",
      how: { kind: "finished" },
      status: { code: 0 },
      at: "2026-10-04T14:52:30Z",
      output: "/home/me/.fastctx/world/towns/mark50/residents/claude-v01c/out/j-r4g7e2.log",
      last: { text: "test result: ok. 42 passed; 0 failed; 0 ignored\n\n   Doc-tests ctrl\ntest result: ok. 3 passed; 0 failed", whole: false },
    },
  );
  s.started("t2");
  s.say("t2", "thinking", "", 2);
  s.tool("t2", run("rg -c 'test result: ok' /home/me/.fastctx/world/towns/mark50/residents/claude-v01c/out/j-r4g7e2.log", "=== run on lab (exited 0) ===", "9", { duration_ms: 60 }, "lab"));
  s.say("t2", "message", "九组测试都过了，step_response 稳态到了 0.9999。");
  s.tool(
    "t2",
    say("整套回归跑完了：42 个单元测试、3 个文档测试全过，step_response 稳态 0.9999（原来 0.9612）。修复在 `fix/q32-integral` 分支上，等你看过再合。", "info", [
      { machine: "lab", path: "/home/me/.fastctx/world/towns/mark50/residents/claude-v01c/out/j-r4g7e2.log" },
    ]),
  );
  s.ended("t2", "completed");

  s.heard("t3", { kind: "world" }, "=== sentinel s-7f2c on server: it holds ===\nThe loss of run3 rose from 0.31 to 4.7 after step 11,200 and keeps rising.", 9800, { kind: "words", from: { kind: "world" }, text: "the training loss diverged" });
  s.started("t3");
  s.say("t3", "thinking", "", 3);
  s.tool("t3", run("tail -5 /data/run3/train.log", "=== run on gpu (exited 0) ===", "step 11180 loss 0.312\nstep 11200 loss 0.334\nstep 11220 loss 1.906\nstep 11240 loss 3.551\nstep 11260 loss 4.702", { duration_ms: 120 }, "gpu"));
  s.tool("t3", run("pkill -INT -f 'train.py --run run3'", "=== run on gpu (exited 0) ===", "", { duration_ms: 300 }, "gpu"));
  s.tool(
    "t3",
    run(
      "cd /data && python train.py --run run3b --from run3/ckpt-11000 --lr 1.5e-4",
      "=== run on gpu (still running; j-w8n3c1) ===",
      "resumed from run3/ckpt-11000\nstep 11020 loss 0.309",
      { duration_ms: 15_000 },
      "gpu",
    ),
  );
  s.say("t3", "message", "停了 run3，从 11k 的检查点以一半的学习率接着跑成 run3b，前一千步看着正常。");
  s.tool(
    "t3",
    say("训练在 11,200 步之后发散了，loss 从 0.31 升到 4.7。我停了它，从第 11k 步的检查点把学习率减半重跑（run3b），前一千步正常。要不要把这个学习率写进默认配置，等你定。", "urgent", [{ machine: "gpu", path: "/data/run3/loss.png" }]),
  );
  s.ended("t3", "completed");

  s.heard("t4", user, "辛苦了。分支里改了几个文件？", 16_000);
  s.started("t4");
  s.tool("t4", run("cd /srv/mark50 && git diff --stat main...fix/q32-integral", "=== run on lab (exited 0) ===", " ctrl/src/fixed.rs     | 14 ++++++++------\n ctrl/tests/vectors.rs | 642 +++++++++++++++++\n 2 files changed, 650 insertions(+), 6 deletions(-)", { duration_ms: 140 }, "lab"));
  s.say("t4", "message", "两个：`ctrl/src/fixed.rs` 把积分项放宽到 Q32，`ctrl/tests/vectors.rs` 是新录的 640 组阶跃响应向量。");
  s.push({ event: "context", used: 72_800, window: 200_000 });
  s.ended("t4", "completed", null, 1, {
    ms: 9_000,
    facts: {
      models: ["claude-opus-5-5"],
      efforts: [{ kind: "requested", value: "high" }],
      tokens: { host: "claude", input_tokens: 1_204, cache_read_input_tokens: 68_310, cache_creation_input_tokens: 2_116, output_tokens: 182 },
      cost: 96_402,
    },
  });

  // A command the user gives from the input begins a turn of their own: the
  // host keeps its line for the command out of the conversation, and the
  // answer, given in plain words again, is still theirs.
  s.push(
    {
      event: "changed",
      turn: null,
      change: {
        kind: "metadata",
        metadata: {
          fastctx_control: {
            id: "control-1791131040000-5-0",
            control: { do: "command", name: "release-notes", args: "" },
            at: "2026-10-04T17:02:00Z",
            state: "completed",
            result: { kind: "reported", said: null },
            error: null,
          },
        } as unknown as JsonValue,
      },
    },
    240,
  );
  s.started("t5");
  s.tool("t5", run("cd /srv/mark50 && git log --oneline main..fix/q32-integral", "=== run on lab (exited 0) ===", "b81e3f2 Widen the integral to Q32\n2d90a1c Record step-response vectors", { duration_ms: 120 }, "lab"));
  s.say("t5", "message", "**0.4.1**\n\n- 积分项放宽到 Q32，阶跃响应不再在 0.8 秒处饱和。\n- 新录 640 组阶跃响应向量，回归覆盖全部工况。");
  s.ended("t5", "completed", null, 1, {
    ms: 9_000,
    facts: {
      models: ["claude-opus-5-5"],
      efforts: [{ kind: "requested", value: "high" }],
      tokens: { host: "claude", input_tokens: 1_204, cache_read_input_tokens: 68_310, cache_creation_input_tokens: 2_116, output_tokens: 182 },
      cost: 96_402,
      limits: {
        observed_at: "2026-10-04T16:58:20Z",
        windows: [
          { name: "seven_day", minutes: 10080, used_tenths: 688, resets_at: "2026-10-08T18:00:00Z" },
          { name: "five_hour", minutes: 300, used_tenths: 236, resets_at: "2026-10-04T19:00:00Z" },
        ],
      },
    },
  });

  return {
    title: "夜里的回归与训练",
    agent: "claude",
    resident: "claude-v01c",
    machine: "laptop",
    model: "Opus 5.5",
    town: "mark50",
    here: "desktop",
    events: s.recent(),
  };
}
