// The specimen's activity books: a few months of deeds on the specimen's
// machines, the busy hours of a working week, now and then a failure, a
// long training run, and what ran this very minute. Seeded, so every
// screenshot shows the same.

import { seeded } from "../pen/shapes";
import type { BookAnswer, BookQuery, Deed, Hour, Touched } from "../world/book";

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

interface Habit {
  machine: string;
  /** Deeds on a busy weekday hour. */
  rate: number;
  sessions: { session: string; agent: "claude" | "codex"; held: boolean; project?: string }[];
  scripts: { script: string; touched?: Touched[]; seconds: number; fails?: number }[];
}

const HABITS: Habit[] = [
  {
    machine: "desktop",
    rate: 6,
    sessions: [
      { session: "claude-3f2a@desktop", agent: "claude", held: true, project: "mark50" },
      { session: "codex-7b1c@desktop", agent: "codex", held: false },
      { session: "codex-a41d@desktop", agent: "codex", held: true, project: "telemetry" },
    ],
    scripts: [
      { script: "on desktop cargo test -p firmware-sim", seconds: 48, fails: 0.18 },
      { script: "on desktop <<'RUN'\ncd ~/mark50/firmware && cargo fmt && cargo clippy --all-targets\nRUN", seconds: 26, touched: [{ path: "C:/Users/me/mark50/firmware/src/radio.rs", how: "changed", added: 4, removed: 4, seen: true }] },
      { script: "on desktop edit C:/Users/me/mark50/firmware/src/radio.rs", seconds: 1, touched: [{ path: "C:/Users/me/mark50/firmware/src/radio.rs", how: "changed", added: 18, removed: 6 }] },
      { script: "on desktop write C:/Users/me/mark50/firmware/src/telemetry/frame.rs", seconds: 1, touched: [{ path: "C:/Users/me/mark50/firmware/src/telemetry/frame.rs", how: "added", added: 64, removed: 0 }] },
      { script: "on desktop rg -n \"TX_POWER\" ~/mark50/firmware", seconds: 1 },
      { script: "on desktop fastctx view C:/Users/me/mark50/notes/bringup.pdf --pages 3-5", seconds: 2 },
      { script: "on desktop git -C ~/mark50 log --oneline -20", seconds: 1 },
      { script: "on desktop npm --prefix ~/mark50/dashboard run build", seconds: 74, fails: 0.1, touched: [{ path: "C:/Users/me/mark50/dashboard/dist/index.html", how: "changed", added: 3, removed: 3, seen: true }] },
    ],
  },
  {
    machine: "server",
    rate: 3,
    sessions: [{ session: "claude-3f2a@desktop", agent: "claude", held: true, project: "telemetry" }],
    scripts: [
      { script: "on server <<'RUN'\ncd /srv/telemetry && journalctl -u ingest --since -1h | tail -80\nRUN", seconds: 3 },
      { script: "on server systemctl restart ingest", seconds: 4, fails: 0.08 },
      { script: "on server edit /srv/telemetry/ingest/config.toml", seconds: 1, touched: [{ path: "/srv/telemetry/ingest/config.toml", how: "changed", added: 2, removed: 1 }] },
      { script: "on server df -h /srv && du -sh /srv/telemetry/*", seconds: 2 },
    ],
  },
  {
    machine: "gpu",
    rate: 0.6,
    sessions: [{ session: "codex-a41d@desktop", agent: "codex", held: true, project: "telemetry" }],
    scripts: [
      { script: "on gpu <<'RUN'\ncd ~/runs && python train.py --epochs 3 --batch 64\nRUN", seconds: 2600, fails: 0.2, touched: [{ path: "/home/bo/runs/checkpoints/epoch-3.pt", how: "added" }] },
      { script: "on gpu nvidia-smi", seconds: 1 },
    ],
  },
  {
    machine: "laptop",
    rate: 1.2,
    sessions: [{ session: "codex-91aa@laptop", agent: "codex", held: false }],
    scripts: [
      { script: "on laptop pytest -q tests/test_parser.py", seconds: 12, fails: 0.25 },
      { script: "on laptop copy server:/srv/telemetry/out/day.csv C:/Users/me/data/", seconds: 6, touched: [{ path: "C:/Users/me/data/day.csv", how: "copied" }] },
    ],
  },
];

/** How busy an hour is, by when it falls: working days and hours. */
function busy(at: number): number {
  const date = new Date(at);
  const weekday = (date.getDay() + 6) % 7;
  const hour = date.getHours();
  const day = weekday < 5 ? 1 : 0.55;
  const time = hour >= 9 && hour < 19 ? 1 : hour >= 19 && hour < 24 ? 0.5 : hour < 3 ? 0.3 : 0.12;
  return day * time;
}

function deedsOf(habit: Habit, from: number, to: number, now: number): Deed[] {
  const deeds: Deed[] = [];
  const first = Math.max(from, now - 90 * DAY);
  for (let hour = Math.floor(first / HOUR) * HOUR; hour < Math.min(to, now); hour += HOUR) {
    const random = seeded(`${habit.machine}:${hour}`);
    const count = Math.floor(habit.rate * busy(hour) * (0.4 + random() * 1.2));
    for (let index = 0; index < count; index += 1) {
      const pick = habit.scripts[Math.floor(random() * habit.scripts.length)]!;
      const session = habit.sessions[Math.floor(random() * habit.sessions.length)]!;
      const start = hour + Math.floor(random() * HOUR);
      if (start < from || start >= to || start > now) continue;
      const seconds = pick.seconds * (0.6 + random() * 0.8);
      const end = start + seconds * 1000;
      const running = end > now;
      const failed = !running && random() < (pick.fails ?? 0.02);
      const stopped = !running && !failed && random() < 0.01;
      deeds.push({
        job: `j-${(hour / HOUR + index).toString(36).slice(-6).padStart(6, "0")}`,
        began: new Date(start).toISOString(),
        ...(running ? {} : { ended: new Date(end).toISOString() }),
        ...(session.session.endsWith(`@${habit.machine}`) ? {} : { from: session.session.slice(session.session.indexOf("@") + 1) }),
        session: session.session,
        agent: session.agent,
        held: session.held,
        ...(session.project ? { project: session.project } : {}),
        script: pick.script,
        outcome: running ? "running" : failed ? "failed" : stopped ? "stopped" : "done",
        ...(running ? {} : { code: failed ? 1 : 0 }),
        output: Math.floor(200 + random() * (pick.seconds > 1000 ? 12_000_000 : 40_000)),
        looked: pick.script.includes("view") ? 1 : 0,
        copied: pick.script.includes("copy") ? 1 : 0,
        ...(pick.touched && !failed ? { touched: pick.touched } : {}),
      });
    }
  }
  // Something runs this very minute on the computer and the server.
  if (now >= from && now < to + HOUR) {
    if (habit.machine === "desktop") {
      deeds.push({ job: "j-runnow", began: new Date(now - 140_000).toISOString(), session: "claude-3f2a@desktop", agent: "claude", held: true, project: "mark50", script: "on desktop cargo test -p firmware-sim -- --include-ignored", outcome: "running", output: 18_200, looked: 0, copied: 0 });
    }
    if (habit.machine === "gpu") {
      deeds.push({ job: "j-trains", began: new Date(now - 41 * 60_000).toISOString(), from: "desktop", session: "codex-a41d@desktop", agent: "codex", held: true, project: "telemetry", script: "on gpu <<'RUN'\ncd ~/runs && python train.py --epochs 3 --batch 64\nRUN", outcome: "running", output: 2_400_000, looked: 0, copied: 0 });
    }
  }
  return deeds;
}

function hoursOf(habit: Habit, from: number, to: number, now: number): Hour[] {
  const hours: Hour[] = [];
  for (let hour = Math.floor(Math.max(from, now - 366 * DAY) / HOUR) * HOUR; hour < Math.min(to, now); hour += HOUR) {
    const random = seeded(`${habit.machine}:${hour}`);
    // The months further back were quieter.
    const age = (now - hour) / (366 * DAY);
    const runs = Math.floor(habit.rate * busy(hour) * (0.4 + random() * 1.2) * (1 - age * 0.6));
    if (!runs) continue;
    const session = habit.sessions[Math.floor(random() * habit.sessions.length)]!;
    hours.push({
      at: new Date(hour).toISOString(),
      session: session.session,
      agent: session.agent,
      held: session.held,
      runs,
      failed: random() < 0.12 ? 1 : 0,
      busy: runs * Math.floor(20_000 + random() * 60_000),
      files: random() < 0.3 ? Math.ceil(random() * 3) : 0,
    });
  }
  return hours;
}

/** The book of `machine`, or of the specimen's own computer. */
export async function specimenBook(machine: string | null, query: BookQuery, now: number): Promise<BookAnswer> {
  const name = machine ?? "desktop";
  const habit = HABITS.find((one) => one.machine === name);
  if (!habit) throw new Error(`${name} did not answer`);
  const from = Date.parse(query.from);
  const to = Date.parse(query.to);
  if (query.ask === "hours") return { kind: "hours", machine: name, hours: hoursOf(habit, from, to, now) };
  const deeds = deedsOf(habit, from, to, now).sort((a, b) => Date.parse(b.began) - Date.parse(a.began));
  return { kind: "deeds", machine: name, deeds: deeds.slice(0, query.limit), more: deeds.length > query.limit };
}

/** The machines the specimen's page asks: the lab is off. */
export const specimenBookMachines = [
  { written: null, name: "desktop", online: true },
  { written: "laptop", name: "laptop", online: true },
  { written: "server", name: "server", online: true },
  { written: "gpu", name: "gpu", online: true },
  { written: "lab", name: "lab", online: false },
];
