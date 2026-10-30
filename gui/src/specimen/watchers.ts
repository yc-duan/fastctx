// The watchers the specimen conversation left (`?sentinels`), and what each
// did, as their page reads it: one waits for the training loss, three wakes
// so far, the last still under way (with `?found`, it found it); one waits
// for a time tomorrow and has looked once; one was only just left.

import type { SessionEvent } from "../world/events";
import type { JsonValue } from "../world/wire";
import type { SentinelOverview, SentinelPhase, SentinelRecord, SentinelSummary, SentinelWake, SentinelWatched } from "../world/sentinels";

const LOSS = "s-a1b2c3";
const MORNING = "s-d4e5f6";
const FRESH = "s-0f9e8d";

/** Tomorrow at `hour`. */
function tomorrow(now: number, hour: number): number {
  const at = new Date(now);
  at.setDate(at.getDate() + 1);
  at.setHours(hour, 0, 0, 0);
  return at.getTime();
}

export function specimenWatchers(english: boolean, now: number, found: boolean): SentinelSummary[] {
  const say = (zh: string, en: string) => (english ? en : zh);
  const one = (id: string, condition: string, plan: string, phase: SentinelPhase, at?: number[]): SentinelSummary => ({
    definition: { id, home: "server", condition, kind: "local", owner: { machine: "laptop", resident: "claude-7a1c" }, generation: 1, ...(at ? { at } : {}) },
    phase,
    plan,
    latest: null,
    reason: null,
  });
  return [
    one(
      LOSS,
      say("训练的 loss 降到 0.3 以下时叫醒我", "Wake me when the training loss falls below 0.3"),
      say("每两分钟读一次 train.log 的最后几行", "Reads the last lines of train.log every two minutes"),
      found ? "satisfied" : "watching",
    ),
    one(
      MORNING,
      say("明早八点叫醒我看夜里的回归", "Wake me at eight tomorrow to read the night's regression"),
      say("只等时刻，之前什么都不看", "Waits for the time; looks at nothing before it"),
      "watching",
      [tomorrow(now, 8), tomorrow(now, 20)],
    ),
    one(FRESH, say("部署的健康检查连续三次失败时叫醒我", "Wake me when the deploy's health check fails three times in a row"), "", "starting"),
  ];
}

export function specimenWatcherRecord(english: boolean, now: number, id: string, found: boolean): SentinelRecord {
  const summary = specimenWatchers(english, now, found).find((one) => one.definition.id === id);
  const say = (zh: string, en: string) => (english ? en : zh);
  const at = (minutesAgo: number) => new Date(now - minutesAgo * 60_000).toISOString();
  const events: SessionEvent[] = [];
  let item = 0;
  const wake = (turn: string, minutesAgo: number, why: JsonValue, seen = "") => {
    events.push({ at: at(minutesAgo), event: "turn_started", turn } as SessionEvent);
    events.push({
      at: at(minutesAgo),
      event: "heard",
      delivery: null,
      turn,
      from: { kind: "world" },
      text: seen,
      notice: { kind: "world", fact: { fact: "woke", why }, text: seen },
    } as SessionEvent);
  };
  const finished = (turn: string, minutesAgo: number, what: Record<string, JsonValue>) => {
    item += 1;
    events.push({ at: at(minutesAgo), event: "finished", turn, item: { id: `${turn}-${item}`, ...what } } as SessionEvent);
  };
  const look = (turn: string, minutesAgo: number, command: string, output: string, running = false) =>
    finished(turn, minutesAgo, {
      kind: "tool",
      name: "look",
      server: null,
      machine: "server",
      does: { as: "command", command, cwd: null, exit: null, job: null },
      input: { machine: "server", command },
      output: running ? [] : [{ type: "text", text: output }],
      status: running ? "running" : "done",
      duration_ms: null,
    });
  const reflex = (turn: string, minutesAgo: number, every: number, command: string) =>
    finished(turn, minutesAgo, {
      kind: "tool",
      name: "install_reflex",
      server: null,
      machine: null,
      does: { as: "call" },
      input: { id: "loss", every_ms: every * 60_000, probe: { kind: "look", machine: "server", command } },
      output: [{ type: "text", text: '{"installed":true}' }],
      status: "done",
      duration_ms: null,
    });
  const message = (turn: string, minutesAgo: number, text: string) => finished(turn, minutesAgo, { kind: "message", text });
  const ended = (turn: string, minutesAgo: number) =>
    events.push({ at: at(minutesAgo), event: "turn_ended", turn, end: { how: "completed", reason: null, duration_ms: 9000 } } as SessionEvent);

  if (id === MORNING) {
    wake("w1", 42, { kind: "started" });
    message("w1", 42, say("只需等到明早八点，此前无需巡检。", "Only the time tomorrow at eight matters; nothing needs checking before it."));
    ended("w1", 42);
    return { version: "specimen", events, ...(summary ? { summary } : {}), next: tomorrow(now, 8) };
  }
  if (id !== LOSS) return { version: "specimen", events, ...(summary ? { summary } : {}) };

  const tail = "tail -n 3 /data/run3/train.log";
  wake("w1", 95, { kind: "started" });
  finished("w1", 95, { kind: "thinking", text: say("先看日志现在走到哪一步，再决定巡检的间隔。", "First see where the log stands, then decide how often to check.") });
  look("w1", 95, tail, "step 4100  loss 0.431\nstep 4150  loss 0.427\nstep 4200  loss 0.412");
  reflex("w1", 94, 10, tail);
  message("w1", 94, say("当前 0.412，距 0.3 尚远。已设定每十分钟读取最后几行的巡检，低于 0.34 时再唤醒复核。", "0.412 now, far from 0.3. A check reads the last lines every ten minutes and wakes me below 0.34."));
  ended("w1", 94);

  wake("w2", 31, { kind: "reflex", source: `server: ${tail}` }, "step 6150  loss 0.338");
  look("w2", 31, tail, "step 6100  loss 0.341\nstep 6150  loss 0.338\nstep 6200  loss 0.333");
  reflex("w2", 30, 2, tail);
  message("w2", 30, say("0.333，仍在下降。巡检间隔改为两分钟，低于 0.3 即报告。", "0.333 and still falling. Checking every two minutes now; below 0.3 it reports."));
  ended("w2", 30);

  wake("w3", 1, { kind: "reflex", source: `server: ${tail}` }, "step 7350  loss 0.297");
  finished("w3", 1, { kind: "thinking", text: say("巡检读到 0.297，再亲自读取一次，排除一时的波动。", "The check saw 0.297; read once more myself so it is not a passing dip.") });
  if (!found) {
    look("w3", 0, tail, "", true);
    return { version: "specimen", events, ...(summary ? { summary } : {}), next: now + 2 * 60_000 };
  }
  look("w3", 0, tail, "step 7350  loss 0.297\nstep 7400  loss 0.291\nstep 7450  loss 0.288");
  // What it reports is its words.
  message("w3", 0, say("连续三步低于 0.3（0.297、0.291、0.288），条件成立，唤醒对话。", "Three steps in a row under 0.3 (0.297, 0.291, 0.288): it holds, and the conversation is woken."));
  ended("w3", 0);
  return { version: "specimen", events, ...(summary ? { summary } : {}) };
}

/** Every watcher of the specimen's account (`?sentinels`), over its last
 *  days: the three the conversation left still watching, and four that
 *  ended, two of which found what they waited for. */
export function specimenWatchersOverview(english: boolean, now: number): SentinelOverview {
  const say = (zh: string, en: string) => (english ? en : zh);
  const hour = 3_600_000;
  const wake = (ago: number, kind: SentinelWake["why"]["kind"], tokens = 9_000, failed = false): SentinelWake => ({
    at: now - ago * hour,
    why: kind === "reflex" ? { kind, source: "server:tail -n3 train.log" } : kind === "broken" ? { kind, reason: "" } : ({ kind } as SentinelWake["why"]),
    input: tokens,
    output: Math.round(tokens / 12),
    cached: Math.round(tokens * 0.7),
    requests: 2,
    elapsed_ms: 18_000 + (tokens % 7) * 3_000,
    ...(failed ? { failed } : {}),
  });
  const summary = (id: string, condition: string, phase: SentinelPhase, home = "server"): SentinelSummary => ({
    definition: { id, home, condition, kind: "local", owner: { machine: "laptop", resident: "claude-7a1c" }, generation: 1 },
    phase,
    plan: "",
    latest: null,
    reason: null,
  });
  const live = specimenWatchers(english, now, false);
  const watched: SentinelWatched[] = [
    { summary: live[0]!, wakes: [wake(70, "started", 12_000), wake(40, "reflex"), wake(22, "reflex"), wake(9, "reflex"), wake(2.5, "reflex", 14_000)] },
    { summary: live[1]!, wakes: [wake(14, "started", 6_000)] },
    { summary: live[2]!, wakes: [wake(0.2, "started", 5_000)] },
    {
      summary: summary("s-7c6b5a", say("CI 的夜间构建变绿时叫醒我", "Wake me when the nightly CI build goes green"), "satisfied"),
      wakes: [wake(130, "started"), wake(118, "reflex"), wake(104, "reflex", 11_000)],
    },
    {
      summary: summary("s-5a4b3c", say("磁盘用到九成时叫醒我", "Wake me when the disk is nine tenths full"), "satisfied", "lab"),
      wakes: [wake(160, "started"), wake(96, "due"), wake(60, "reflex", 13_000)],
    },
    {
      summary: summary("s-3c2d1e", say("对方的接口恢复时叫醒我", "Wake me when their API answers again"), "unable", "lab"),
      wakes: [wake(52, "started"), wake(47, "reflex"), wake(45, "broken", 4_000, true)],
    },
    {
      summary: summary("s-1e0f9a", say("周五下午提醒我交周报", "Remind me on Friday afternoon to send the weekly report"), "withdrawn"),
      wakes: [wake(30, "started", 3_000), wake(28, "withdrawn", 2_000)],
    },
  ];
  return { watched, unavailable: [] };
}
