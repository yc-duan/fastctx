// `?s=stream&start=agents`: the host's own agents in a conversation, as
// Claude Code starts them; with `&host=codex`, as Codex spawns its threads.
// Done before: a turn that sent three agents out at once, one of which
// failed, each one's own record read as its box opens. Then, played at a
// model's pace: one agent that goes on in the background and one the turn
// waits on, which starts one of its own; later the background one ends,
// the conversation is told, and a turn says what it found.

import type { AgentActivity, Content, Does, HostAgent, HostAgentEnd, HostAgentStart, Happened, Item, SessionEvent, Speaker, Tool } from "../world/events";
import type { Stream } from "./streams";

type Host = "claude" | "codex";
type Call = Partial<Tool> & { name: string; does: Does };

const user: Speaker = { kind: "user", machine: "desktop" };
const text = (value: string): Content[] => [{ type: "text", text: value }];

/** Each agent's own record, as its host keeps it: what its box reads when
 *  the agent was not seen from its start. What lies ahead of the specimen's
 *  clock is not in it yet. */
const records = new Map<string, SessionEvent[]>();

export async function specimenAgentRecord(agent: string): Promise<AgentActivity> {
  await new Promise((resolve) => setTimeout(resolve, 280));
  const events = records.get(agent);
  if (!events) return { events: [], unavailable: `no agent ${agent} in this specimen` };
  const now = Date.now();
  return { events: events.filter((event) => Date.parse(event.at) <= now) };
}

/** Events with a time cursor of their own: each loop, the conversation's
 *  and each agent's, is written on its own and all are merged by time. */
class Loop {
  readonly events: SessionEvent[] = [];
  private serial = 0;

  constructor(
    private at: number,
    private readonly prefix: string,
  ) {}

  get now(): number {
    return this.at;
  }

  /** Moves this loop's clock to a moment, where it is behind it. */
  until(at: number): void {
    this.at = Math.max(this.at, at);
  }

  wait(ms: number): void {
    this.at += ms;
  }

  id(): string {
    this.serial += 1;
    return `${this.prefix}-${this.serial}`;
  }

  push(happened: Happened, after = 0): void {
    this.at += after;
    this.events.push({ at: new Date(this.at).toISOString(), ...happened } as SessionEvent);
  }

  heard(turn: string | null, said: string, after = 0): void {
    this.push({ event: "heard", delivery: null, turn, from: user, text: said }, after);
  }

  started(turn: string, after = 300): void {
    this.push({ event: "turn_started", turn }, after);
  }

  think(turn: string, said: string, ms: number): void {
    const id = this.id();
    this.push({ event: "began", turn, item: { id, started_at: new Date(this.at + 200).toISOString(), kind: "thinking", text: "" } }, 200);
    this.push({ event: "finished", turn, item: { id, kind: "thinking", text: said, duration_ms: ms } }, ms);
  }

  /** A tool called and, unless `running`, ended: its id. */
  tool(turn: string, call: Call, ms: number, after = 400, running = false): string {
    const id = this.id();
    this.begin(turn, id, call, after);
    if (!running) this.finish(turn, id, call, ms);
    return id;
  }

  begin(turn: string, id: string, call: Call, after = 400): void {
    const began = this.at + after;
    this.push({ event: "began", turn, item: { id, started_at: new Date(began).toISOString(), kind: "tool", ...tool(call), status: "running", output: [] } as Item }, after);
  }

  finish(turn: string, id: string, call: Call, ms: number): void {
    this.push({ event: "finished", turn, item: { id, kind: "tool", ...tool(call) } as Item }, ms);
  }

  say(turn: string, said: string, pace = 22): void {
    const id = this.id();
    this.push({ event: "began", turn, item: { id, kind: "message", text: "" } }, 300);
    for (const piece of pieces(said)) this.push({ event: "grew", turn, item: id, part: "text", text: piece }, pace);
    this.push({ event: "finished", turn, item: { id, kind: "message", text: said } }, 40);
  }

  ended(turn: string, ms: number, answer: string | null, how = "completed", reason: string | null = null): void {
    this.push({ event: "turn_ended", turn, end: { how, reason, duration_ms: ms, last_message: answer } }, 200);
  }

  changed(change: Extract<Happened, { event: "changed" }>["change"], turn: string | null = null, after = 0): void {
    this.push({ event: "changed", turn, change }, after);
  }
}

function tool(call: Call): Tool {
  return { server: "fastctx", machine: null, input: {}, output: [], status: "done", duration_ms: null, ...call };
}

function pieces(said: string): string[] {
  const out: string[] = [];
  for (let at = 0; at < said.length; at += 6) out.push(said.slice(at, at + 6));
  return out;
}

/** A World command, as its host reports one. */
function run(machine: string, command: string, body: string, ms: number, code = 0): Call {
  return {
    name: "run",
    machine,
    input: { command },
    output: text(`=== run on ${machine} (exited ${code}) ===\n${body}`),
    does: { as: "command", command, cwd: null, exit: code, job: null },
    status: code === 0 ? "done" : "failed",
    duration_ms: ms,
  };
}

/** What one agent does in its own loop, written once: as its record and,
 *  for one followed live, as what came of it as it happened. */
interface Work {
  id: string;
  start: HostAgentStart;
  /** Its own loop's events. */
  own: Loop;
  end: HostAgentEnd;
}

/** One agent's own loop: its turn, its thinking, its steps, its answer. */
function work(
  host: Host,
  at: number,
  id: string,
  start: Omit<HostAgentStart, "id" | "live">,
  steps: { think?: string; call?: Call; ms: number }[],
  answer: string,
  failed: string | null = null,
): Work {
  const own = new Loop(at, id);
  const turn = `${id}:1`;
  own.started(turn, 200);
  let tools = 0;
  for (const step of steps) {
    if (step.think !== undefined) own.think(turn, host === "codex" ? "" : step.think, step.ms);
    if (step.call) {
      own.tool(turn, step.call, step.ms, 300, false);
      tools += 1;
    }
  }
  if (failed) own.ended(turn, own.now - at, null, "failed", failed);
  else {
    own.say(turn, answer, 14);
    own.ended(turn, own.now - at, answer);
  }
  const end: HostAgentEnd = failed
    ? { how: "failed", reason: failed, duration_ms: own.now - at, tools }
    : { how: "completed", duration_ms: own.now - at, tokens: 18_000 + tools * 4_100, tools, answer };
  return { id, start: { id, ...start, live: false }, own, end };
}

/** An agent's own events as the conversation is told them while they
 *  happen. */
function told(agent: Work): SessionEvent[] {
  return agent.own.events.map(({ at, ...happened }) => ({
    at,
    event: "changed",
    turn: null,
    change: { kind: "agent_event", agent_id: agent.id, happened: happened as Happened },
  }));
}

/** The call that starts an agent, as each host makes it. */
function starting(host: Host, agent: Work, background: boolean, answer: string | null, failed = false): Call {
  const { kind, task, prompt } = agent.start;
  if (host === "codex") {
    return {
      name: "spawnAgent",
      server: null,
      input: { prompt: prompt ?? null, model: agent.start.model ?? null },
      does: { as: "agent", act: "start", agents: [agent.id], kind: null, task: prompt ?? null },
      status: "done",
    };
  }
  return {
    name: "Agent",
    server: null,
    input: { description: task ?? null, prompt: prompt ?? null, subagent_type: kind ?? null, ...(background ? { run_in_background: true } : {}) },
    output: text(
      background
        ? `Async agent launched successfully.\nagentId: ${agent.id} (internal ID - do not mention to user. Use SendMessage with to: '${agent.id}' to continue this agent.)`
        : (answer ?? ""),
    ),
    does: { as: "agent", act: "start", agents: [agent.id], kind: kind ?? null, task: task ?? null },
    status: failed ? "failed" : "done",
  };
}

const ROUNDING = `在 ctrl/、bus/、joints/ 里找出所有「先在 Q16 里累加、再乘增益」的写法。
这种写法会让小误差在累加时被舍掉，step_response 就是这样停在 0.9612 的。
只读不改；每处给出文件、行号和一句话说明为什么会舍掉精度。`;

export function agentsSpecimen(host: Host): Stream {
  records.clear();
  const codex = host === "codex";
  const model = codex ? "gpt-6.1-sol" : "claude-opus-5-5";
  const small = codex ? "gpt-6-luna" : "claude-haiku-5-5";
  const start = Date.now() - 12 * 60_000;
  const main = new Loop(start, "item");

  // Before: three agents at once, one of which failed.
  main.heard("t1", "ctrl、bus、joints 三块分头查一下：哪里还有 Q16 先累加后乘增益的写法？各派一个子代理去查，查完汇总给我。");
  main.started("t1");
  main.think("t1", codex ? "" : "**分头查**\n\n三块互不相干，各派一个子代理并行查，我来汇总。", 2600);
  const areas: { area: string; name: string; found: string; failed?: string }[] = [
    { area: "ctrl", name: "Noether", found: "ctrl/src/fixed.rs:58 `step()` 在 Q16 里累加积分再乘 Ki：误差小于 2⁻¹⁶ 时整步被舍掉，这就是 0.9612 的来由。\nctrl/src/pid.rs 用的是 f32，没有这个问题。" },
    { area: "bus", name: "Hopper", found: "bus/ 里只有 can.rs:204 的时间戳滤波做了 Q16 累加，但它先乘系数再累加，不会舍掉小量。没发现同样的问题。" },
    { area: "joints", name: "Lovelace", found: "", failed: codex ? "stream disconnected before completion: model overloaded" : "API Error: 529 {\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}" },
  ];
  const first: Work[] = areas.map(({ area, name, found, failed }, index) =>
    work(
      host,
      main.now + 800 + index * 300,
      codex ? `019a${index}e7c-5d1b-7c40-9e1a-agent${index}` : `a${index + 1}f3c9e2b7d`,
      {
        kind: codex ? "explorer" : "Explore",
        ...(codex ? { name } : {}),
        task: `查 ${area} 的 Q16 舍入`,
        prompt: ROUNDING.replace("ctrl/、bus/、joints/", `${area}/`),
        model: small,
        background: codex,
      },
      [
        { think: `**Scanning ${area}/**\n\nLooking for integrators summed before the gain.`, ms: 1800 },
        { call: run("laptop", `rg -n 'Q16|widen|narrow' /home/me/mark50/src/${area}`, `/home/me/mark50/src/${area}/mod.rs:12:use crate::q16::Q16;\n…`, 140), ms: 900 },
        { call: run("laptop", `fastctx view -- /home/me/mark50/src/${area}/${area === "bus" ? "can.rs" : "fixed.rs"}`, "=== view (1 file: 1 shown) ===\n…", 210), ms: 1400 },
        ...(failed ? [] : [{ think: "**Checking the order**\n\nIs the gain applied before or after the sum?", ms: 2400 }, { call: run("laptop", `rg -n 'fn step' /home/me/mark50/src/${area}`, "…", 90), ms: 700 }]),
      ],
      found,
      failed ?? null,
    ),
  );
  for (const agent of first) records.set(agent.id, agent.own.events);
  const calls = first.map((agent, index) => ({ agent, call: starting(host, agent, codex, areas[index]!.found || null, !!areas[index]!.failed) }));
  if (codex) {
    // Codex spawns each, then waits on all of them at once.
    for (const { agent, call } of calls) {
      const id = main.tool("t1", call, 400, 300);
      main.changed({ kind: "agent_started", agent: { ...agent.start, call: id } }, "t1");
    }
    const waited = main.id();
    const wait: Call = { name: "wait", server: null, input: { ids: first.map((agent) => agent.id) }, does: { as: "agent", act: "wait", agents: first.map((agent) => agent.id), kind: null, task: null } };
    main.begin("t1", waited, wait, 300);
    main.until(Math.max(...first.map((agent) => agent.own.now)));
    main.finish("t1", waited, wait, 200);
    for (const agent of first) main.changed({ kind: "agent_ended", agent_id: agent.id, end: uncounted(agent.end) }, "t1");
  } else {
    // Claude Code starts all three in one response; each call ends as its
    // agent does, and its end is read from the call's result.
    const ids = calls.map(({ call }, index) => {
      const id = main.id();
      main.begin("t1", id, call, index ? 120 : 300);
      return id;
    });
    const order = calls.map((one, index) => ({ ...one, id: ids[index]! })).sort((a, b) => a.agent.own.now - b.agent.own.now);
    for (const { agent, call, id } of order) {
      main.until(agent.own.now);
      main.finish("t1", id, call, 100);
      main.changed({ kind: "agent_started", agent: { ...agent.start, call: id } }, "t1");
      main.changed({ kind: "agent_ended", agent_id: agent.id, end: agent.end }, "t1");
    }
  }
  main.think("t1", codex ? "" : "**汇总**\n\njoints 那个失败了，自己补查一下。", 1900);
  main.tool("t1", run("laptop", "rg -n 'Q16' /home/me/mark50/src/joints", "/home/me/mark50/src/joints/limits.rs:31:    let span = Q16::from(max - min);", 120), 400);
  main.say(
    "t1",
    "三块查完了：\n\n- **ctrl**：`fixed.rs:58` 的 `step()` 先在 Q16 里累加积分再乘 Ki，误差小于 2⁻¹⁶ 时整步被舍掉，就是它让 step_response 停在 0.9612。\n- **bus**：`can.rs:204` 先乘系数再累加，没问题。\n- **joints**：子代理没跑完（服务过载），我自己补查了：`limits.rs` 只做比较不累加，没问题。\n\n要改的只有 ctrl 一处。",
    8,
  );
  main.ended("t1", main.now - start, null);
  const events = main.events;

  // Played live from a moment after the page opens.
  const live = new Loop(Date.now() + 1500, "live");
  live.heard("t2", "bus 的 CAN 帧时间戳也让一个子代理在后台慢慢查，查完告诉我；joints/limits.rs 的限幅现在就看，看完说要不要改。");
  live.started("t2");
  live.think("t2", codex ? "" : "**一快一慢**\n\n时间戳那个放后台；限幅这个等它看完再答。", 2200);
  const later: SessionEvent[] = [];

  // The one that goes on in the background.
  const slow = work(
    host,
    live.now + 900,
    codex ? "019a7e7c-5d1b-7c40-9e1a-agentbg" : "b7c1d9e4f2",
    {
      kind: codex ? "explorer" : "general-purpose",
      ...(codex ? { name: "Hopper" } : {}),
      task: "查 CAN 帧时间戳的漂移",
      prompt: "bus/can.rs 的帧时间戳在长时间运行后会漂吗？在 lab 上抓 30 秒 candump，对比时间戳与本机时钟，给出漂移量和原因。",
      model: codex ? small : model,
      background: true,
    },
    [
      { think: "**Reading the timestamp path**\n\nWhere does a frame get its time?", ms: 2600 },
      { call: run("laptop", "fastctx view -- /home/me/mark50/src/bus/can.rs", "=== view (1 file: 1 shown) ===\n…", 180), ms: 1600 },
      { call: run("lab", "timeout 30 candump -ta can0 | tail -n 4", "(1760262401.120384) can0 18FF50E5#0102030405060708\n(1760262401.130391) can0 18FF50E5#0102030405060708\n…", 30_000), ms: 30_400 },
      { think: "**Comparing clocks**\n\n7 µs per frame, linear: the divider, not jitter.", ms: 3200 },
      { call: run("lab", "chronyc tracking | head -n 3", "Reference ID    : C0A80101 (gateway)\nStratum         : 3\nRef time (UTC)  : …", 400), ms: 900 },
    ],
    "漂移是线性的：每帧约 7 µs，30 秒累计 21 ms。原因在 `can.rs:198`：时间戳用 72 MHz 的计数器除以 72 取整，每次舍掉的小数从不补回。改成按 Q32 累加余数即可，不必换时钟源。",
  );
  // The one the turn waits on, which starts one of its own.
  const limits = codex ? "019a7e7c-5d1b-7c40-9e1a-agentfg" : "c3e8a1f5b0";
  const nested = codex ? "019a7e7c-5d1b-7c40-9e1a-agentnx" : "d9f2b6c4a7";
  const quick = new Loop(live.now + 1400, limits);
  const quickTurn = `${limits}:1`;
  quick.started(quickTurn, 200);
  quick.think(quickTurn, codex ? "" : "**Reading the limits**\n\nClamp first, then check whether anything accumulates.", 1500);
  quick.tool(quickTurn, run("laptop", "fastctx view -- /home/me/mark50/src/joints/limits.rs", "=== view (1 file: 1 shown) ===\n…", 160), 900, 300);
  // It sends one of its own to measure on lab while it reads on.
  const inner = work(
    host,
    quick.now + 700,
    nested,
    {
      parent: limits,
      kind: codex ? "worker" : "general-purpose",
      ...(codex ? { name: "Ramanujan" } : {}),
      task: "在 lab 上量限幅的饱和点",
      prompt: "在 lab 上跑 joints 的 limit_sweep，报告各关节实际的饱和点与设定值的偏差。",
      model: small,
      background: false,
    },
    [
      { call: run("lab", "cd /srv/mark50 && cargo run --release -p joints --example limit_sweep", "joint 0: set ±2.500 rad, saturates at ±2.49997\njoint 1: set ±1.750 rad, saturates at ±1.74998\n…", 6200), ms: 6600 },
    ],
    "六个关节的饱和点与设定值相差都在 4×10⁻⁵ rad 以内，是 Q16 的一个最小单位，限幅本身没有累积误差。",
  );
  const spawnCall: Call = codex
    ? { name: "spawnAgent", server: null, input: { prompt: inner.start.prompt ?? null }, does: { as: "agent", act: "start", agents: [nested], kind: null, task: inner.start.prompt ?? null } }
    : { name: "Agent", server: null, input: { description: inner.start.task ?? null, prompt: inner.start.prompt ?? null, subagent_type: inner.start.kind ?? null }, output: text(inner.end.answer ?? ""), does: { as: "agent", act: "start", agents: [nested], kind: inner.start.kind ?? null, task: inner.start.task ?? null } };
  const spawnId = quick.id();
  quick.begin(quickTurn, spawnId, spawnCall, 500);
  inner.start.call = spawnId;
  quick.tool(quickTurn, run("laptop", "rg -n 'clamp|saturat' /home/me/mark50/src/joints", "/home/me/mark50/src/joints/limits.rs:44:        value.clamp(self.min, self.max)", 110), 600, 400);
  quick.until(inner.own.now);
  quick.finish(quickTurn, spawnId, spawnCall, codex ? 100 : 200);
  quick.think(quickTurn, codex ? "" : "**Deciding**\n\nNo accumulation, saturation within one LSB: leave it.", 1700);
  const quickAnswer = "`joints/limits.rs` 的限幅只做 `clamp` 比较、不累加，lab 上实测的饱和点与设定值相差不到 Q16 的一个最小单位。不用改。";
  quick.say(quickTurn, quickAnswer, 14);
  quick.ended(quickTurn, quick.now - (live.now + 1400), quickAnswer);
  const quickWork: Work = {
    id: limits,
    start: {
      id: limits,
      kind: codex ? "explorer" : "Explore",
      ...(codex ? { name: "Lovelace" } : {}),
      task: "看 joints 限幅要不要改",
      prompt: "读 joints/limits.rs 的限幅实现，判断它有没有和 ctrl 一样的 Q16 累加舍入问题；需要实测就派人去 lab 量。最后一句话给结论：要不要改。",
      model: small,
      background: codex,
      live: true,
    },
    own: quick,
    end: { how: "completed", duration_ms: quick.now - (live.now + 1400), tokens: 41_230, answer: quickAnswer },
  };
  for (const agent of [slow, inner]) records.set(agent.id, agent.own.events);
  // Its own record tells of the agent it started as its host reads it:
  // started and ended where the call that started it ended.
  const quickRecord = [...quick.events];
  const spawned = quickRecord.findIndex((event) => event.event === "finished" && event.item.id === spawnId);
  const spawnedAt = quickRecord[spawned]!.at;
  quickRecord.splice(
    spawned + 1,
    0,
    { at: spawnedAt, event: "changed", turn: quickTurn, change: { kind: "agent_started", agent: { ...inner.start, live: false } } },
    { at: spawnedAt, event: "changed", turn: quickTurn, change: { kind: "agent_ended", agent_id: nested, end: inner.end } },
  );
  records.set(limits, quickRecord);

  // The calls that start them, and what the conversation is told of them.
  const slowCall = starting(host, slow, true, null);
  const quickCall = starting(host, quickWork, false, quickAnswer);
  const slowId = live.tool("t2", slowCall, 300, 300);
  live.changed({ kind: "agent_started", agent: { ...slow.start, call: slowId, live: true } }, "t2");
  if (!codex) live.changed({ kind: "agents", agents: [listed(slow, "running")] });
  const quickId = live.id();
  live.begin("t2", quickId, quickCall, 200);
  if (codex) live.finish("t2", quickId, quickCall, 300);
  live.changed({ kind: "agent_started", agent: { ...quickWork.start, call: quickId } }, "t2");
  later.push(...told(slow), ...told(quickWork));
  later.push({ at: new Date(inner.own.events[0]!.at).toISOString(), event: "changed", turn: null, change: { kind: "agent_started", agent: { ...inner.start, live: true } } });
  later.push(...told(inner));
  later.push({ at: new Date(inner.own.now + 50).toISOString(), event: "changed", turn: null, change: { kind: "agent_ended", agent_id: nested, end: uncounted(inner.end) } });
  later.push({ at: new Date(quick.now + 50).toISOString(), event: "changed", turn: null, change: { kind: "agent_ended", agent_id: limits, end: uncounted(quickWork.end) } });
  if (codex) {
    // Codex waits on the one it needs now, and says one thing more to the
    // other while it waits.
    const asked = live.tool(
      "t2",
      { name: "sendInput", server: null, input: { message: "顺便看一下 can.rs 的滤波窗口" }, does: { as: "agent", act: "message", agents: [slow.id], kind: null, task: "顺便看一下 can.rs 的滤波窗口" } },
      200,
      2600,
    );
    void asked;
    const waited = live.id();
    const wait: Call = { name: "wait", server: null, input: { ids: [limits] }, does: { as: "agent", act: "wait", agents: [limits], kind: null, task: null } };
    live.begin("t2", waited, wait, 300);
    live.until(quick.now + 120);
    live.finish("t2", waited, wait, 100);
  } else {
    live.until(quick.now + 120);
    live.finish("t2", quickId, quickCall, 100);
  }
  live.say("t2", "限幅不用改：`limits.rs` 只做比较不累加，子代理在 lab 上量过，饱和点和设定值差不到一个 Q16 最小单位。\n\nCAN 时间戳那个还在后台查，查完我告诉你。", 20);
  live.ended("t2", live.now - (Date.now() + 1500), null);
  // The background one ends; the conversation is told, and says so.
  later.push({ at: new Date(slow.own.now + 60).toISOString(), event: "changed", turn: null, change: { kind: "agent_ended", agent_id: slow.id, end: uncounted(slow.end) } });
  const after = new Loop(Math.max(slow.own.now + 400, live.now + 400), "after");
  if (codex) {
    // Codex hears of it as an activity of the agent's thread, and its next
    // turn takes it in.
    after.started("t3");
    after.changed({ kind: "agent_ended", agent_id: slow.id, end: { how: "completed", ...(slow.end.answer ? { answer: slow.end.answer } : {}) } }, "t3", 100);
  } else {
    after.changed({ kind: "agents", agents: [listed(slow, "completed")] }, null, 50);
    after.changed({ kind: "agent_ended", agent_id: slow.id, end: { ...slow.end, ...(slow.start.task ? { task: slow.start.task } : {}), told: true } }, null, 100);
    after.started("t3");
  }
  after.think("t3", codex ? "" : "**Passing it on**\n\nThe drift is the divider's remainder.", 1600);
  after.say("t3", "后台那个查完了：CAN 时间戳每帧漂约 7 µs，30 秒累计 21 ms。原因在 `can.rs:198`，72 MHz 计数器除以 72 时舍掉的小数从不补回。改成按 Q32 累加余数就行，不用换时钟源。要我改吗？", 20);
  after.ended("t3", after.now - slow.own.now, null);
  later.push(...live.events, ...after.events);
  later.sort((a, b) => Date.parse(a.at) - Date.parse(b.at));

  return {
    title: codex ? "分头查 Q16 舍入" : "分头查 Q16 舍入",
    agent: host,
    resident: "stream-1",
    machine: "laptop",
    model,
    town: "mark50",
    here: "desktop",
    events,
    later,
    rate: 0,
    timed: true,
  };
}

/** An end as a live host tells it, before its tools are counted. */
function uncounted(end: HostAgentEnd): HostAgentEnd {
  const { tools: _tools, ...rest } = end;
  return rest;
}

/** How Claude Code's own list says one of its agents stands. */
function listed(agent: Work, status: string): HostAgent {
  return { id: agent.id, type: agent.start.kind ?? "general-purpose", description: agent.start.task ?? "", status } as HostAgent;
}
