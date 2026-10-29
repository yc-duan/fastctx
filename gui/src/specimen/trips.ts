// A specimen conversation whose runs go about the network
// (`world/run-facts.ts`, `RunStep`): data read on one machine, fitted on another and
// written back, the result copied to a third and tested there; a check sent
// to a dozen computers of a lab, two of which did not end well; and a file
// looked at and changed on a server through `on`. Each run carries the
// steps the World recorded as it did them.

import type { Content, Does, SessionEvent, Speaker, Tool } from "../world/events";
import type { RunFacts, RunStep, StepDid, WriteFact } from "../world/run-facts";
import { cut, hunk, put, same, wrote } from "./run-facts";
import type { Specimen } from "./sessions";

const text = (value: string): Content[] => [{ type: "text", text: value }];

export function trips(): Specimen {
  const events: SessionEvent[] = [];
  let clock = Date.now() - 18 * 60_000;
  let serial = 0;
  const at = (seconds: number) => {
    clock += seconds * 1000;
    return new Date(clock).toISOString();
  };
  const id = () => `trip-${(serial += 1)}`;
  const turn = "t1";
  const user: Speaker = { kind: "user", machine: "desktop" };
  events.push({
    at: at(0),
    event: "heard",
    delivery: null,
    turn: null,
    from: user,
    text: "把 laptop 上的阶跃数据拿到 desktop 上拟合，结果放回 laptop，再同步到 lab 跑一遍控制器测试；顺便看看实验楼 3 层那批机器的 git 都装好没有。",
  });
  events.push({ at: at(1), event: "turn_started", turn });
  events.push({ at: at(2), event: "finished", turn, item: { id: id(), kind: "thinking", text: "**先跑数据，再测**\n\n拟合在 desktop 上做，数据经管道直接流过去。", duration_ms: 3400 } });

  /** A run of `command` on the desktop, its steps as the World told them. */
  const run = (command: string, steps: RunStep[], writes: WriteFact[], output: string, seconds: number) => {
    const item = id();
    const began = at(2);
    const facts: RunFacts = { machine: "desktop", state: { kind: "ended", status: { code: 0 } }, job: `j-${item.slice(-4)}`, output: null, visited: [], writes, steps, spans: [{ piece: { kind: "text", first_line: 1, text: output } }] };
    const tool: Tool = {
      server: "fastctx",
      name: "run",
      machine: "desktop",
      input: { command },
      output: text(`=== run on desktop (exited 0) ===\n${output}`),
      status: "done",
      duration_ms: seconds * 1000,
      does: { as: "command", command, cwd: null, exit: 0, job: null } as Does,
      run: facts,
    };
    events.push({ at: began, event: "began", turn, item: { id: item, started_at: began, kind: "tool", ...tool, status: "running", output: [] } });
    events.push({ at: at(seconds), event: "finished", turn, item: { id: item, kind: "tool", ...tool } });
  };
  // Steps' times, as seconds after now on the specimen's clock.
  const when = (from: number, to: number | null) => {
    const began = new Date(clock + from * 1000).toISOString();
    return to === null ? { began } : { began, ended: new Date(clock + to * 1000).toISOString() };
  };
  /** A step the job `job` on `machine` told. */
  const step = (id: string, job: string, machine: string, from: number, to: number | null, did: StepDid): RunStep => ({ id, job, machine, ...when(from, to), ...did });

  run(
    [
      "cd ~/mark50 && git pull --ff-only",
      "on laptop cat /home/me/mark50/data/steps.csv \\",
      "  | on desktop python C:/work/fit.py --model q32 \\",
      "  | on laptop tee /home/me/mark50/data/fit.csv > /dev/null",
      "fastctx copy laptop:/home/me/mark50/data/fit.csv lab:/srv/mark50/data/",
      "on lab cargo test -p ctrl --release 2>&1 | tail -n 3",
    ].join("\n"),
    [
      step("s1", "j-r1", "desktop", 1, 3, { kind: "on", to: "laptop", command: "cat /home/me/mark50/data/steps.csv", sent: "j-a1", end: { kind: "exited", code: 0 }, pipes_to: "desktop" }),
      step("s2", "j-r1", "desktop", 1.2, 6, { kind: "on", to: "desktop", command: "python C:/work/fit.py --model q32", sent: "j-a2", end: { kind: "exited", code: 0 }, fed_by: "j-a1", pipes_to: "laptop", tail: "fit: kp=1.84 ki=0.37 kd=0.052  residual 0.0031" }),
      step("s3", "j-r1", "desktop", 1.4, 6.2, { kind: "on", to: "laptop", command: "tee /home/me/mark50/data/fit.csv > /dev/null", sent: "j-a3", end: { kind: "exited", code: 0 }, fed_by: "j-a2" }),
      step("s4", "j-r1", "desktop", 7, 8, { kind: "copy", from: [{ machine: "laptop", path: "/home/me/mark50/data/fit.csv" }], to: { machine: "lab", path: "/srv/mark50/data/" }, files: 1, bytes: 48_210, unchanged: 0 }),
      step("s5", "j-r1", "desktop", 8, 51, {
        kind: "on",
        to: "lab",
        command: "cargo test -p ctrl --release 2>&1",
        sent: "j-a5",
        end: { kind: "exited", code: 0 },
        tail: "test fixed::tests::step_response ... ok\n\ntest result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.12s",
      }),
    ],
    [],
    "Already up to date.\ntest result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.12s",
    52,
  );

  events.push({ at: at(3), event: "finished", turn, item: { id: id(), kind: "message", text: "拟合好了，结果已经放回 laptop 并同步到 lab，lab 上 42 个测试全过。接着看实验楼 3 层的机器。" } });

  const lab = Array.from({ length: 12 }, (_, n) => `实验楼3层-AG${String(n + 1).padStart(2, "0")}`);
  run(
    `for m in ${lab.join(" ")}; do\n  on "$m" git --version &\ndone\nwait`,
    lab.map((machine, n): RunStep =>
      step(`f${n + 1}`, "j-r2", "desktop", 1 + n * 0.05, n === 8 ? 21 : 3 + (n % 4) * 0.4, {
        kind: "on",
        to: machine,
        command: "git --version",
        sent: `j-f${n + 1}`,
        end: n === 4 ? { kind: "exited", code: 127 } : n === 8 ? { kind: "lost", why: "the machine went offline" } : { kind: "exited", code: 0 },
        ...(n === 4 ? { tail: "bash: git: command not found" } : n === 8 ? {} : { tail: "git version 2.56.0.windows.2" }),
      }),
    ),
    [],
    "git version 2.56.0.windows.2\n…",
    22,
  );

  const rows = [hunk(128, 128), same(129, 129, "    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {"), cut(130, "        self.integral += error * dt;"), put(130, "        self.integral = self.integral.saturating_add(error.widen() * dt.widen());"), same(131, 131, "        let p = self.kp * error;")];
  run(
    [
      "on lab fastctx view /srv/mark50/src/ctrl/fixed.rs",
      "on lab edit /srv/mark50/src/ctrl/fixed.rs <<'EOF'",
      "<<<<<<< SEARCH",
      "        self.integral += error * dt;",
      "=======",
      "        self.integral = self.integral.saturating_add(error.widen() * dt.widen());",
      ">>>>>>> REPLACE",
      "EOF",
    ].join("\n"),
    [
      step("v1", "j-r3", "desktop", 1, 2, { kind: "on", to: "lab", command: "fastctx view /srv/mark50/src/ctrl/fixed.rs", sent: "j-v1", end: { kind: "exited", code: 0 } }),
      step("v2", "j-v1", "lab", 1.1, 1.2, { kind: "view", paths: ["/srv/mark50/src/ctrl/fixed.rs"] }),
      step("v3", "j-r3", "desktop", 3, 4, { kind: "on", to: "lab", command: "edit /srv/mark50/src/ctrl/fixed.rs <<'EOF'", sent: "j-v3", end: { kind: "exited", code: 0 } }),
    ],
    [wrote("9e1f20aa", 0, "lab", "/srv/mark50/src/ctrl/fixed.rs", "modified", rows)],
    "=== edit lab:/srv/mark50/src/ctrl/fixed.rs (+1 −1) ===",
    5,
  );

  events.push({ at: at(3), event: "finished", turn, item: { id: id(), kind: "message", text: "实验楼 3 层 12 台里 10 台装好了；AG05 还没装 git，AG09 跑到一半离线了，要我给它们补装吗？" } });
  events.push({ at: at(1), event: "turn_ended", turn, end: { how: "completed", reason: null, duration_ms: 95_000, last_message: null } });

  // A second turn still going: the install done on one machine, still
  // running on the other.
  const going = "t2";
  clock = Date.now() - 30_000;
  events.push({ at: at(0), event: "heard", delivery: null, turn: null, from: user, text: "补装吧。" });
  events.push({ at: at(1), event: "turn_started", turn: going });
  const install = "winget install --id Git.Git -e --silent";
  const item = id();
  const began = at(2);
  const steps: RunStep[] = [
    step("g1", "j-going", "desktop", 0.5, 9, { kind: "on", to: lab[4]!, command: install, sent: "j-g1", end: { kind: "exited", code: 0 }, tail: "Successfully installed" }),
    step("g2", "j-going", "desktop", 0.6, null, { kind: "on", to: lab[8]!, command: install, sent: "j-g2" }),
  ];
  const command = `for m in ${lab[4]} ${lab[8]}; do\n  on "$m" ${install} &\ndone\nwait`;
  const facts: RunFacts = { machine: "desktop", state: { kind: "running" }, job: "j-going", output: null, visited: [], writes: [], steps, spans: [] };
  events.push({
    at: began,
    event: "began",
    turn: going,
    item: { id: item, started_at: began, kind: "tool", server: "fastctx", name: "run", machine: "desktop", input: { command }, output: [], status: "running", duration_ms: null, does: { as: "command", command, cwd: null, exit: null, job: null } as Does, run: facts },
  });
  return { title: "拟合与测试", agent: "codex", resident: "trips-1", machine: "desktop", model: "gpt-6.1-sol", town: "mark50", here: "desktop", events };
}
