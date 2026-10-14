// A long specimen conversation for the steps of its context
// (`conversation/context-steps.ts`): a Claude session of some fifty
// requests, each answer naming the tools it called, each tool's result
// growing the next prompt by as much as it returned, and one compaction on
// the way, so what every step added can be read and drawn.

import type { Content, Does, SessionEvent, Speaker, Tool } from "../world/events";
import type { Specimen } from "./sessions";

const text = (value: string): Content[] => [{ type: "text", text: value }];

/** Roughly how many characters a token holds, in code and its output. */
const CHARS = 3.6;

interface Call {
  name: string;
  does: Does;
  input: { [key: string]: string };
  /** What it returned, in characters. */
  size: number;
}

const view = (path: string, size: number): Call => ({
  name: "run",
  does: { as: "command", command: `fastctx view ${path}`, cwd: null, exit: 0, job: null },
  input: { command: `fastctx view ${path}` },
  size,
});

const command = (line: string, size: number, description?: string): Call => ({
  name: "run",
  does: { as: "command", command: line, cwd: null, exit: 0, job: null },
  input: description ? { command: line, description } : { command: line },
  size,
});

const edit = (path: string, size: number): Call => ({
  name: "Edit",
  does: { as: "edit", changes: [{ path, how: "updated", diff: "@@ -1,2 +1,2 @@\n-let i = acc.narrow();\n+let i = acc.widen();\n" }] },
  input: { file_path: path },
  size,
});

/** What each answer called, in order; an empty answer only spoke. */
const plan: Call[][] = [
  [view("src/ctrl/fixed.rs", 9_800), view("src/ctrl/pid.rs", 6_400)],
  [command("rg -n integral src", 2_300)],
  [view("tests/step_response.rs", 4_100)],
  [command("cargo test -p ctrl step_response", 18_600, "跑 step_response")],
  [view("src/ctrl/q16.rs", 7_200)],
  [],
  [edit("src/ctrl/fixed.rs", 900)],
  [command("cargo test -p ctrl", 21_400, "跑整套测试")],
  [view("logs/ctrl-test.log", 26_000)],
  [command("rg -n saturating src/ctrl", 1_200)],
  [edit("src/ctrl/fixed.rs", 700), edit("src/ctrl/q16.rs", 600)],
  [command("cargo build -p ctrl", 3_400)],
  [],
  [view("docs/fixed-point.md", 12_500)],
  [command("cargo test -p ctrl overflow", 5_200)],
  [edit("tests/step_response.rs", 1_100)],
  [command("cargo test -p ctrl", 20_900, "跑整套测试")],
  [command("git diff --stat", 800)],
  [],
  [view("src/ctrl/limits.rs", 5_600), view("src/ctrl/clamp.rs", 3_900)],
  [command("rg -n clamp src", 2_900)],
  [edit("src/ctrl/limits.rs", 800)],
  [command("cargo clippy -p ctrl", 6_700)],
  [command("cargo test -p ctrl limits", 4_800)],
  [],
  [view("benches/loop.rs", 4_400)],
  [command("cargo bench -p ctrl --bench loop", 15_800, "跑基准")],
  [edit("benches/loop.rs", 500)],
  [command("cargo bench -p ctrl --bench loop", 15_200, "再跑一次基准")],
  [],
  [view("src/ctrl/fixed.rs", 10_400)],
  [command("cargo test -p ctrl", 21_000, "跑整套测试")],
  [command("rg -n TODO src/ctrl", 1_500)],
  [edit("src/ctrl/pid.rs", 900)],
  [command("cargo test -p ctrl pid", 5_900)],
  [],
  [command("on lab cargo test -p ctrl", 22_800, "在 lab 上跑整套测试")],
  [view("logs/lab-test.log", 17_400)],
  [edit("src/ctrl/fixed.rs", 600)],
  [command("on lab cargo test -p ctrl", 22_100, "在 lab 上再跑一次")],
  [command("git status --short", 400)],
  [],
];

/** The answer before which the conversation was compacted, and to what
 *  share of its context. */
const COMPACT = 33;
const KEPT = 0.32;

function filler(size: number, seed: number): string {
  const lines: string[] = [];
  let length = 0;
  let line = seed;
  while (length < size) {
    line += 1;
    const said = `${String(line).padStart(4, " ")} | let q${line % 97} = acc.saturating_add(err * KI_${line % 13}); // step ${line % 31}`;
    lines.push(said);
    length += said.length + 1;
  }
  return lines.join("\n").slice(0, size);
}

export function steps(): Specimen {
  const events: SessionEvent[] = [];
  let clock = Date.now() - 46 * 60_000;
  let serial = 0;
  const at = (seconds: number) => {
    clock += seconds * 1000;
    return new Date(clock).toISOString();
  };
  const id = () => `steps-${(serial += 1)}`;
  const turn = "t1";
  const user: Speaker = { kind: "user", machine: "desktop" };
  events.push({
    at: at(0),
    event: "heard",
    delivery: null,
    turn: null,
    from: user,
    text: "lab 上 ctrl 的测试一直挂在 step_response，帮我查清楚、修好，修完在 lab 上把整套测试跑一遍。",
  });
  events.push({ at: at(1), event: "turn_started", turn });
  let context = 18_400;
  for (let index = 0; index < plan.length; index += 1) {
    const calls = plan[index]!;
    const ids = calls.map(() => id());
    const output = 180 + ((index * 977) % 900) + calls.length * 140;
    // The cache serves what it held; what is new since is written to it.
    const write = index === 0 || index === COMPACT ? Math.round(context * 0.96) : Math.min(context, 600 + ((index * 431) % 2_400));
    const fresh = Math.round(20 + ((index * 53) % 60));
    const read = Math.max(0, context - write - fresh);
    if (index === COMPACT) events.push({ at: at(2), event: "finished", turn, item: { id: id(), kind: "compaction" } });
    if (!calls.length) {
      const said = { id: id(), kind: "message", text: index === plan.length - 1 ? "修好了：积分改在 Q32 里累加、只在乘上 Ki 之后收窄回 Q16；lab 上整套测试全部通过。" : "测试过了这一段，接着往下查。" } as const;
      events.push({ at: at(4), event: "began", turn, item: { ...said, text: "" } });
      events.push({ at: at(2), event: "finished", turn, item: said });
    }
    events.push({
      at: at(calls.length ? 3 : 1),
      event: "changed",
      turn,
      change: {
        kind: "message_usage",
        items: ids,
        response: `msg_steps_${index}`,
        model: "claude-opus-5-5",
        api_error: false,
        usage: { input_tokens: fresh, cache_read_input_tokens: read, cache_creation_input_tokens: write, cache_creation: { ephemeral_1h_input_tokens: write, ephemeral_5m_input_tokens: 0 }, output_tokens: output },
      },
    });
    let returned = 0;
    calls.forEach((call, at_) => {
      const tool: Tool = { server: "fastctx", machine: "laptop", input: call.input, output: [], status: "running", duration_ms: null, name: call.name, does: call.does };
      const began = at(1 + (at_ % 2));
      events.push({ at: began, event: "began", turn, item: { id: ids[at_]!, started_at: began, kind: "tool", ...tool } });
      events.push({ at: at(2 + Math.round(call.size / 6_000)), event: "finished", turn, item: { id: ids[at_]!, kind: "tool", ...tool, status: "done", output: text(filler(call.size, index * 100 + at_)) } });
      returned += Math.round(call.size / CHARS);
    });
    // The next prompt holds this answer, what its tools returned, and the
    // World's few words about them; a compaction keeps a part of it.
    context = context + output + returned + 40 + ((index * 17) % 90);
    if (index + 1 === COMPACT) context = Math.round(context * KEPT);
  }
  events.push({ at: at(2), event: "turn_ended", turn, end: { how: "completed", reason: null, duration_ms: null, last_message: null } });
  return { title: "修 step_response", agent: "claude", resident: "claude-5teps", machine: "laptop", model: "claude-opus-5-5", town: "mark50", here: "desktop", events };
}
