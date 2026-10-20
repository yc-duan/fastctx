// Conversations made to measure, for looking at what only size and speed
// show: a history of thousands of tool calls, a turn streaming as fast as a
// model's fastest tier, a conversation still starting and one that could
// not start. `?s=stream` opens one; the address picks its shape:
// `turns`, `tools` (calls per turn), `rate` (events a second, live),
// `start` (`starting` or `failed`; `think` for thinking, `agents` for the
// host's own agents, `&host=codex` for Codex's).

import type { Content, Item, SessionEvent, Speaker, Tool } from "../world/events";
import type { Specimen } from "./sessions";
import { agentsSpecimen } from "./agents";

/** A specimen that plays on after its history, with its own pace. */
export interface Stream extends Specimen {
  /** What happens after the history, played live. */
  later: SessionEvent[];
  /** Events played a second. */
  rate: number;
  /** Played at the times the events carry, instead of at `rate`. */
  timed?: boolean;
}

const user: Speaker = { kind: "user", machine: "desktop" };

const text = (value: string): Content[] => [{ type: "text", text: value }];

class Clock {
  private at: number;
  constructor(start: number) {
    this.at = start;
  }
  next(ms: number): string {
    this.at += ms;
    return new Date(this.at).toISOString();
  }
}

const paths = ["src/ctrl/fixed.rs", "src/ctrl/pid.rs", "src/joints/limits.rs", "src/q16.rs", "tests/step.rs", "Cargo.toml", "README.md", "src/bus/can.rs"];

/** One tool call, begun and finished. */
function tool(clock: Clock, turn: string, id: string, n: number): SessionEvent[] {
  const path = paths[n % paths.length]!;
  const kind = n % 5;
  const command =
    kind === 0
      ? `rg -n 'fn step' /home/me/mark50/${path}`
      : kind === 1
        ? `fastctx view -- /home/me/mark50/${path}`
        : kind === 2
          ? `cd /srv/mark50 && cargo test -p ctrl ${n}`
          : kind === 3
            ? `git -C /home/me/mark50 diff --stat`
            : `ls /home/me/mark50/src`;
  const output = Array.from({ length: 3 + (n % 9) }, (_, line) => `${path}:${line + 1}: ${"x".repeat(20 + ((n * 7 + line) % 60))}`).join("\n");
  const full: Tool = {
    name: "run",
    server: "fastctx",
    machine: "laptop",
    input: { command },
    output: text(`=== run on laptop (exited 0) ===\n${output}`),
    does: { as: "command", command, cwd: null, exit: 0, job: null },
    status: "done",
    duration_ms: 300 + (n % 7) * 200,
  };
  const began = clock.next(400);
  return [
    { at: began, event: "began", turn, item: { id, started_at: began, kind: "tool", ...full, status: "running", output: [] } },
    { at: clock.next(full.duration_ms!), event: "finished", turn, item: { id, kind: "tool", ...full } },
  ];
}

const answer = (n: number) => `第 ${n} 轮看完了。\`fixed.rs\` 里的积分项改成在 **Q32** 里累加，只在乘上 $K_i$ 之后才收窄。

- 稳态误差从 0.0388 降到 0.0001；
- \`saturating_add\` 让长时间饱和时不回绕。

\`\`\`rust
pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {
    self.integral = self.integral.saturating_add(error.widen() * dt.widen());
    (self.ki.widen() * self.integral).narrow()
}
\`\`\`

下一步在 lab 上把整套测试跑一遍。`;

/** A long answer said a few characters at a time, as a fast tier streams. */
const streamed = `好，我把整个控制环从头到尾过了一遍，下面是我看到的和改了什么。

## 问题在哪

积分项 $\\int e\\,dt$ 用 Q16 累加，每一步 $e \\cdot \\Delta t$ 都被舍到 $2^{-16}$。误差越接近零，被舍掉的比例越大，所以系统停在 0.9612。

## 改动

1. 积分改在 **Q32** 里累加；
2. 只在乘上 $K_i$ 之后才收窄回 Q16；
3. 累加用 \`saturating_add\`，不会回绕。

\`\`\`rust
impl Pid {
    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {
        let p = self.kp * error;
        self.integral = self.integral.saturating_add(error.widen() * dt.widen());
        let i = (self.ki.widen() * self.integral).narrow();
        let d = self.kd * (error - self.last) / dt;
        self.last = error;
        p + i + d
    }
}
\`\`\`

| 参数 | 改前 | 改后 |
|---|---|---|
| 积分精度 | Q16 | Q32 |
| 稳态值 | 0.9612 | 0.9999 |

## 验证

在 lab 上把 \`cargo test --workspace\` 跑了一遍，42 个测试全过；step_response 现在停在 0.9999，误差在 ±0.002 之内。另外把 640 组阶跃向量写进了 \`tests/step_vectors.rs\`，以后改控制环时这组向量会先挂。

还有一处我没动：\`joints/limits.rs\` 里的限幅也是 Q16，但它不累加，舍入误差不会积起来，留着没问题。`;

/** Splits words the way a model streams them: a few characters each. */
function pieces(said: string): string[] {
  const out: string[] = [];
  for (let at = 0; at < said.length; ) {
    const size = 2 + ((at * 13) % 5);
    out.push(said.slice(at, at + size));
    at += size;
  }
  return out;
}

/** The first words a specimen conversation is begun with. */
export const FIRST = "帮我看看 lab 上 ctrl 的测试为什么一直挂在 step_response。";

export function streamSpecimen(params: URLSearchParams): Stream {
  if (params.get("start") === "agents") return agentsSpecimen(params.get("host") === "codex" ? "codex" : "claude");
  const turns = Number(params.get("turns") ?? 0);
  const perTurn = Number(params.get("tools") ?? 40);
  const rate = Number(params.get("rate") ?? 0);
  const start = params.get("start");
  // Played live, a begun conversation's events are timed from now.
  const clock = new Clock(start === "open" ? Date.now() - 2400 : Date.now() - 60_000 - turns * perTurn * 1500 - 120_000);
  const events: SessionEvent[] = [];
  let serial = 0;
  const id = () => `item-${++serial}`;
  for (let n = 1; n <= turns; n += 1) {
    const turn = `t${n}`;
    events.push({ at: clock.next(3000), event: "heard", delivery: null, turn, from: user, text: `第 ${n} 件事：把 ctrl 的测试跑一遍，挂了就修。` });
    events.push({ at: clock.next(500), event: "turn_started", turn });
    const thought = id();
    events.push({ at: clock.next(300), event: "finished", turn, item: { id: thought, kind: "thinking", text: `**看一眼测试**\n\n先跑第 ${n} 组，看哪一个挂。` } as Item });
    for (let k = 0; k < perTurn; k += 1) events.push(...tool(clock, turn, id(), n * perTurn + k));
    events.push({ at: clock.next(800), event: "finished", turn, item: { id: id(), kind: "message", text: answer(n) } as Item });
    events.push({
      at: clock.next(200),
      event: "turn_ended",
      turn,
      end: { how: "completed", reason: null, duration_ms: perTurn * 1400, last_message: null },
    });
  }
  const later: SessionEvent[] = [];
  if (start === "think") return thinking();
  if (start === "open") {
    // Begun from the new conversation's page: the first words came with
    // it; the agent takes them in, thinks, and answers.
    const turn = "t1";
    later.push({ at: clock.next(2400), event: "heard", delivery: null, turn, from: user, text: FIRST });
    later.push({ at: clock.next(300), event: "turn_started", turn });
    const thought = id();
    const thinking = "**读测试**\n\n先看 step_response 挂在哪一步。";
    later.push({ at: clock.next(900), event: "began", turn, item: { id: thought, kind: "thinking", text: "" } as Item });
    for (const piece of pieces(thinking)) later.push({ at: clock.next(5), event: "grew", turn, item: thought, part: "text", text: piece });
    later.push({ at: clock.next(50), event: "finished", turn, item: { id: thought, kind: "thinking", text: thinking } as Item });
    for (let k = 0; k < 3; k += 1) later.push(...tool(clock, turn, id(), k));
    const message = id();
    const said = "找到了：积分项在 Q16 里累加，误差越小被舍掉的越多，所以停在 0.9612。我把它改成在 Q32 里累加，正在 lab 上重跑。";
    later.push({ at: clock.next(100), event: "began", turn, item: { id: message, kind: "message", text: "" } as Item });
    for (const piece of pieces(said)) later.push({ at: clock.next(5), event: "grew", turn, item: message, part: "text", text: piece });
    later.push({ at: clock.next(50), event: "finished", turn, item: { id: message, kind: "message", text: said } as Item });
  } else if (start) {
    // The first words, and nothing yet from the agent.
    events.push({ at: clock.next(500), event: "heard", delivery: null, turn: null, from: user, text: FIRST });
    if (start === "failed") {
      later.push({
        at: clock.next(4000),
        event: "turn_ended",
        turn: "",
        end: { how: "interrupted", reason: "the Claude command failed before identifying a consuming turn: Claude Code exited: Not logged in · Please run /login", duration_ms: null, last_message: null },
      });
    }
  } else if (rate) {
    const turn = `t${turns + 1}`;
    later.push({ at: clock.next(2000), event: "heard", delivery: null, turn, from: user, text: "把控制环从头到尾过一遍，讲清楚改了什么。" });
    later.push({ at: clock.next(400), event: "turn_started", turn });
    const thought = id();
    later.push({ at: clock.next(100), event: "began", turn, item: { id: thought, kind: "thinking", text: "" } as Item });
    for (const piece of pieces("**读控制环**\n\n先把 fixed.rs 与 pid.rs 对照着读一遍，看积分项在哪里舍入。")) {
      later.push({ at: clock.next(5), event: "grew", turn, item: thought, part: "text", text: piece });
    }
    later.push({ at: clock.next(50), event: "finished", turn, item: { id: thought, kind: "thinking", text: "**读控制环**\n\n先把 fixed.rs 与 pid.rs 对照着读一遍，看积分项在哪里舍入。" } as Item });
    for (let k = 0; k < 6; k += 1) later.push(...tool(clock, turn, id(), k));
    const message = id();
    later.push({ at: clock.next(100), event: "began", turn, item: { id: message, kind: "message", text: "" } as Item });
    for (const piece of pieces(streamed)) later.push({ at: clock.next(5), event: "grew", turn, item: message, part: "text", text: piece });
    later.push({ at: clock.next(50), event: "finished", turn, item: { id: message, kind: "message", text: streamed } as Item });
    later.push({ at: clock.next(100), event: "turn_ended", turn, end: { how: "completed", reason: null, duration_ms: 30_000, last_message: null } });
    // `more`: turns more, played live after it, as an agent left to work
    // for hours goes on.
    for (let n = turns + 2; n <= turns + 1 + Number(params.get("more") ?? 0); n += 1) {
      const next = `t${n}`;
      later.push({ at: clock.next(800), event: "heard", delivery: null, turn: next, from: user, text: `第 ${n} 件事：把 ctrl 的测试跑一遍，挂了就修。` });
      later.push({ at: clock.next(200), event: "turn_started", turn: next });
      for (let k = 0; k < perTurn; k += 1) later.push(...tool(clock, next, id(), n * perTurn + k));
      later.push({ at: clock.next(300), event: "finished", turn: next, item: { id: id(), kind: "message", text: answer(n) } as Item });
      later.push({ at: clock.next(100), event: "turn_ended", turn: next, end: { how: "completed", reason: null, duration_ms: perTurn * 1400, last_message: null } });
    }
  }
  return {
    title: "ctrl 的测试",
    agent: "claude",
    resident: "stream-1",
    machine: "laptop",
    model: "claude-opus-5-5",
    town: "mark50",
    here: "desktop",
    events,
    later,
    rate: rate || (start ? 60 : 0),
  };
}

/** Plays events one task each, `rate` a second (a timer's tick carries as
 *  many as fall due in it), until `take` says to stop. */
export function playLater(events: SessionEvent[], rate: number, take: (event: SessionEvent) => boolean): void {
  if (!rate) return;
  const channel = new MessageChannel();
  let index = 0;
  let due = 0;
  let begun = 0;
  channel.port1.onmessage = () => {
    const event = events[index++];
    if (!event || !take(event)) channel.port1.close();
  };
  const tick = () => {
    begun ||= performance.now();
    const owed = Math.floor(((performance.now() - begun) / 1000) * rate);
    for (; due < owed && due < events.length; due += 1) channel.port2.postMessage(null);
    if (due < events.length) setTimeout(tick, 4);
  };
  setTimeout(tick, 1500);
}

/** A specimen agent answering words sent to it, at a model's pace: it takes
 *  them into a turn, thinks, runs a command and answers, then the turn
 *  ends. */
export function answering(text: string, turn: string): SessionEvent[] {
  const clock = new Clock(Date.now() + 900);
  const thought = `${turn}-think`;
  const ran = `${turn}-run`;
  const said = `${turn}-say`;
  const thinking = "**看一眼限幅**\n\n先读 joints/limits.rs，看限幅在哪里收窄。";
  // "hold" is answered as the World's hold answers once an allowance is used up.
  const held = text.trim() === "hold";
  const until = new Date(Date.now() + 2 * 86_400_000).toISOString().slice(0, 16).replace("T", " ");
  const answer = held
    ? `the weekly allowance of $20.00 this login gives me is used up until ${until} UTC; what is for it waits until then`
    : "看过了：`joints/limits.rs` 里的限幅只做比较、不累加，Q16 的舍入不会积起来，不用改。";
  const events: SessionEvent[] = [
    { at: clock.next(0), event: "heard", delivery: null, turn, from: user, text },
    { at: clock.next(200), event: "turn_started", turn },
    ...(held
      ? []
      : [
          { at: clock.next(500), event: "began", turn, item: { id: thought, kind: "thinking", text: "" } as Item } as SessionEvent,
          ...pieces(thinking).map((piece): SessionEvent => ({ at: clock.next(30), event: "grew", turn, item: thought, part: "text", text: piece })),
          { at: clock.next(100), event: "finished", turn, item: { id: thought, kind: "thinking", text: thinking } as Item } as SessionEvent,
          ...tool(clock, turn, ran, 3),
        ]),
    { at: clock.next(100), event: "began", turn, item: { id: said, kind: "message", text: "" } as Item },
    ...pieces(answer).map((piece): SessionEvent => ({ at: clock.next(30), event: "grew", turn, item: said, part: "text", text: piece })),
    { at: clock.next(100), event: "finished", turn, item: { id: said, kind: "message", text: answer } as Item },
    { at: clock.next(600), event: "turn_ended", turn, end: { how: "completed", reason: null, duration_ms: 6000, last_message: answer } },
  ];
  return events;
}

/** Plays events at the times they carry. */
export function playTimed(events: SessionEvent[], take: (event: SessionEvent) => boolean): void {
  let index = 0;
  const next = () => {
    const event = events[index];
    if (!event) return;
    const due = Date.parse(event.at) - Date.now();
    if (due > 0) {
      setTimeout(next, due);
      return;
    }
    index += 1;
    if (take(event)) next();
  };
  next();
}

/** `?s=stream&start=think`: Claude's way of thinking, its words kept to
 *  itself. Done before: a turn that thought between its commands, and one
 *  that only thought and answered. Then, played at a model's pace, a turn
 *  that thinks a while before anything, runs a command, thinks again, then
 *  thinks aloud and answers. */
function thinking(): Stream {
  let clock = new Clock(Date.now() - 600_000);
  const events: SessionEvent[] = [];
  let serial = 0;
  const id = () => `item-${++serial}`;
  // Thinking as Claude Code tells it: begun, its words where given, its
  // length once over, and the response's thinking tokens as the row is
  // kept.
  const think = (into: SessionEvent[], turn: string, ms: number, said = "", tokens: number | null = null) => {
    const item = id();
    const began = clock.next(300);
    into.push({ at: began, event: "began", turn, item: { id: item, started_at: began, kind: "thinking", text: "" } as Item });
    if (said) {
      const parts = pieces(said);
      for (const piece of parts) into.push({ at: clock.next(Math.round(ms / parts.length)), event: "grew", turn, item, part: "text", text: piece });
    } else clock.next(ms);
    into.push({ at: clock.next(0), event: "finished", turn, item: { id: item, kind: "thinking", text: said, duration_ms: ms } as Item });
    if (tokens !== null) {
      into.push({
        at: clock.next(0),
        event: "changed",
        turn,
        change: { kind: "message_usage", items: [item], response: `msg_${item}`, model: "claude-opus-5-5", api_error: false, usage: { input_tokens: 6, cache_read_input_tokens: 38_000 + serial * 1_350, cache_creation_input_tokens: 900 + serial * 35, cache_creation: { ephemeral_1h_input_tokens: 900 + serial * 35, ephemeral_5m_input_tokens: 0 }, output_tokens: tokens + 120, output_tokens_details: { thinking_tokens: tokens } } },
      });
    }
  };
  const say = (into: SessionEvent[], turn: string, said: string) => {
    const item = id();
    into.push({ at: clock.next(200), event: "began", turn, item: { id: item, kind: "message", text: "" } as Item });
    for (const piece of pieces(said)) into.push({ at: clock.next(20), event: "grew", turn, item, part: "text", text: piece });
    into.push({ at: clock.next(50), event: "finished", turn, item: { id: item, kind: "message", text: said } as Item });
  };
  events.push({ at: clock.next(0), event: "heard", delivery: null, turn: "t1", from: user, text: "看一下 ctrl 的测试为什么挂。" });
  events.push({ at: clock.next(300), event: "turn_started", turn: "t1" });
  think(events, "t1", 4200, "", 312);
  events.push(...tool(clock, "t1", id(), 0));
  think(events, "t1", 2600, "**Reading the integrator**\n\nThe integral is summed in Q16 before the gain; small errors round away.", 1_840);
  events.push(...tool(clock, "t1", id(), 1));
  say(events, "t1", "挂在 step_response：积分项在 Q16 里累加，舍入让它停在 0.9612。");
  events.push({ at: clock.next(100), event: "turn_ended", turn: "t1", end: { how: "completed", reason: null, duration_ms: 12_000, last_message: null } });
  events.push({ at: clock.next(4000), event: "heard", delivery: null, turn: "t2", from: user, text: "要改吗？" });
  events.push({ at: clock.next(300), event: "turn_started", turn: "t2" });
  think(events, "t2", 1800, "", 96);
  say(events, "t2", "要改：在 Q32 里累加，乘上 $K_i$ 之后再收窄。");
  events.push({ at: clock.next(100), event: "turn_ended", turn: "t2", end: { how: "completed", reason: null, duration_ms: 3000, last_message: null } });

  const later: SessionEvent[] = [];
  clock = new Clock(Date.now() + 1200);
  later.push({ at: clock.next(0), event: "heard", delivery: null, turn: "t3", from: user, text: "那就改掉，改完在 lab 上把整套测试跑一遍。" });
  later.push({ at: clock.next(300), event: "turn_started", turn: "t3" });
  think(later, "t3", 4500);
  later.push(...tool(clock, "t3", id(), 3));
  think(later, "t3", 3500);
  later.push(...tool(clock, "t3", id(), 2));
  think(later, "t3", 2500, "**看测试结果**\n\n42 个全过，step_response 停在 0.9999；可以收尾了。");
  say(later, "t3", "改好了：积分在 Q32 里累加，lab 上 42 个测试全过，step_response 停在 0.9999。");
  later.push({ at: clock.next(100), event: "turn_ended", turn: "t3", end: { how: "completed", reason: null, duration_ms: 20_000, last_message: null } });
  return { title: "ctrl 的测试", agent: "claude", resident: "stream-1", machine: "laptop", model: "claude-opus-5-5", town: "mark50", here: "desktop", events, later, rate: 0, timed: true };
}
