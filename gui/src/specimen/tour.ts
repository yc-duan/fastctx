// `?s=tour`: a Claude conversation for pictures of the app, most of what a
// conversation shows in two turns. Asked from the phone, a training run is
// looked at on the machine it ran on; then, asked from the desktop, the
// model it made is brought to the laptop, the firmware changed there and a
// dashboard on a server, the result tested on the rig in the lab. The last
// turn is the full one, so a picture of the conversation's end shows it.

import type { Speaker, TurnFacts } from "../world/events";
import { cut, hunk, noted, printed, put, ran, same, wrote } from "./run-facts";
import { run, Script, sent } from "./script";
import type { Specimen } from "./sessions";

const desk: Speaker = { kind: "user", machine: "desktop" };
const phone: Speaker = { kind: "user", machine: "pixel" };
const FIRMWARE = "C:/Users/me/mark50/firmware";

const facts = (cost: number, input: number, read: number, output: number, used: number): TurnFacts => ({
  models: ["claude-opus-5-5"],
  efforts: [{ kind: "requested", value: "high" }],
  tokens: { host: "claude", input_tokens: input, cache_read_input_tokens: read, cache_creation_input_tokens: 2_816, output_tokens: output },
  cost,
  limits: {
    observed_at: "2026-10-04T14:12:00Z",
    windows: [
      { name: "seven_day", minutes: 10080, used_tenths: used, resets_at: "2026-10-08T18:00:00Z" },
      { name: "five_hour", minutes: 300, used_tenths: 286, resets_at: "2026-10-04T17:00:00Z" },
    ],
  },
});

export function tour(): Specimen {
  const s = new Script("2026-10-04T13:48:00Z");

  s.heard("t1", phone, "gpu 上的前馈模型训完了吗？", 4);
  s.started("t1");
  s.say("t1", "thinking", "", 2);
  const trained = ["epoch 398  loss 0.00412  val_rmse 0.0068", "epoch 399  loss 0.00409  val_rmse 0.0067", "epoch 400  loss 0.00407  val_rmse 0.0067  saved ff.onnx"];
  s.tool(
    "t1",
    run(
      "tail -n 3 /home/bo/runs/ff-1003/train.log",
      "=== run on gpu (exited 0) ===",
      trained.join("\n"),
      { duration_ms: 160, run: ran("gpu", "j-6f9b21", { kind: "ended", status: { code: 0 } }, [printed(1, `${trained.join("\n")}\n`)]) },
      "gpu",
    ),
  );
  s.say("t1", "message", "训完了：400 轮，验证集上的均方根误差降到 0.0067，模型存在 gpu 的 `runs/ff-1003/ff.onnx`。", 3);
  s.ended("t1", "completed", null, 1, { ms: 11_000, facts: facts(38_210, 412, 21_870, 188, 508) });

  s.heard(
    "t2",
    desk,
    "满载时肘关节抖得厉害，示波器截图在附件里。把这个前馈模型接进控制环，server 上的遥测面板也加一栏，改完在 lab 的真机上跑一遍阶跃测试。",
    20 * 60,
    undefined,
    [sent("elbow-scope.png", "image/png", 131_884)],
  );
  s.started("t2");
  s.say("t2", "thinking", "", 2);
  s.tool(
    "t2",
    run(
      `fastctx copy gpu:/home/bo/runs/ff-1003/ff.onnx ${FIRMWARE}/models/`,
      "=== run on laptop (exited 0) ===",
      `=== copy gpu:/home/bo/runs/ff-1003/ff.onnx → laptop:${FIRMWARE}/models/ (1 file, 2.4 MB) ===`,
      { duration_ms: 2100 },
    ),
  );

  const joint = `${FIRMWARE}/src/joint.rs`;
  s.tool(
    "t2",
    run(
      [
        `edit ${joint} <<'EOF'`,
        "<<<<<<< SEARCH",
        "        self.pid.step(error, dt)",
        "=======",
        "        // The torque the load needs before any error builds up;",
        "        // the PID only trims what the model gets wrong.",
        "        let ff = self.model.torque(target, self.velocity, self.load);",
        "        ff + self.pid.step(error, dt)",
        ">>>>>>> REPLACE",
        "EOF",
      ].join("\n"),
      "=== run on laptop (exited 0) ===",
      `=== edit ${joint} (+4 −1) ===`,
      {
        duration_ms: 120,
        run: ran(
          "laptop",
          "j-70a1c3",
          { kind: "ended", status: { code: 0 } },
          [noted("edit", "t0a1c3e5", 1, `=== edit ${joint} (+4 −1) ===\n`)],
          [
            wrote("t0a1c3e5", 0, "laptop", joint, "modified", [
              hunk(56, 56),
              same(56, 56, "    pub fn step(&mut self, target: Angle, dt: Seconds) -> Torque {"),
              same(57, 57, "        let error = target - self.angle;"),
              cut(58, "        self.pid.step(error, dt)"),
              put(58, "        // The torque the load needs before any error builds up;"),
              put(59, "        // the PID only trims what the model gets wrong."),
              put(60, "        let ff = self.model.torque(target, self.velocity, self.load);"),
              put(61, "        ff + self.pid.step(error, dt)"),
              same(59, 62, "    }"),
            ]),
          ],
        ),
      },
    ),
  );

  const panel = "/srv/telemetry/panels/joints.toml";
  s.tool(
    "t2",
    run(
      [
        `on server edit ${panel} <<'EOF'`,
        "<<<<<<< SEARCH",
        'series = ["elbow.torque", "elbow.pid"]',
        "=======",
        'series = ["elbow.torque", "elbow.pid", "elbow.ff"]',
        "stack = true",
        ">>>>>>> REPLACE",
        "EOF",
      ].join("\n"),
      "=== run on laptop (exited 0) ===",
      `=== edit server:${panel} (+2 −1) ===`,
      {
        duration_ms: 340,
        run: ran(
          "laptop",
          "j-70b2d4",
          { kind: "ended", status: { code: 0 } },
          [noted("edit", "t0b2d4f6", 1, `=== edit server:${panel} (+2 −1) ===\n`)],
          [
            wrote("t0b2d4f6", 0, "server", panel, "modified", [
              hunk(12, 12),
              same(12, 12, "[[panel]]"),
              same(13, 13, 'title = "elbow"'),
              cut(14, 'series = ["elbow.torque", "elbow.pid"]'),
              put(14, 'series = ["elbow.torque", "elbow.pid", "elbow.ff"]'),
              put(15, "stack = true"),
            ]),
          ],
        ),
      },
    ),
  );

  s.tool(
    "t2",
    run(
      `cd ${FIRMWARE} && cargo build --release && fastctx copy target/release/firmware.bin lab:/srv/mark50/fw/`,
      "=== run on laptop (exited 0) ===",
      "   Compiling firmware v0.5.0 (C:\\Users\\me\\mark50\\firmware)\n    Finished `release` profile [optimized] target(s) in 38.20s\n=== copy laptop:target/release/firmware.bin → lab:/srv/mark50/fw/ (1 file, 412 KB) ===",
      { duration_ms: 39_400 },
    ),
  );

  const stepTest = [
    "flashed /srv/mark50/fw/firmware.bin to elbow controller (412 KB, 1.8s)",
    "step 0° → 60°, full load, 20 runs",
    "overshoot   3.1%   (was 18.4%)",
    "settling    96 ms  (was 412 ms)",
    "jitter rms  0.11°  (was 0.82°)",
  ];
  s.tool(
    "t2",
    run(
      "/srv/mark50/bin/step-test --joint elbow --load full --flash /srv/mark50/fw/firmware.bin",
      "=== run on lab (exited 0) ===",
      stepTest.join("\n"),
      { duration_ms: 21_600, run: ran("lab", "j-70c3e5", { kind: "ended", status: { code: 0 } }, [printed(1, `${stepTest.join("\n")}\n`)]) },
      "lab",
    ),
  );
  s.say(
    "t2",
    "message",
    `前馈接上了，抖动基本消失。lab 的真机满载跑了 20 次阶跃：

| 指标 | 改前 | 改后 |
|---|---|---|
| 超调 | 18.4% | 3.1% |
| 稳定时间 | 412 ms | 96 ms |
| 抖动（RMS） | 0.82° | 0.11° |

力矩先取模型的估计，PID 只修模型没算准的那一点。server 的遥测面板多了 \`elbow.ff\` 一栏，和 PID 的输出叠在一起看。`,
    3,
  );
  s.push({ event: "context", used: 48_200, window: 200_000 });
  s.ended("t2", "completed", null, 1, { ms: 72_000, facts: facts(386_120, 1_904, 38_416, 1_122, 512) });

  return {
    title: "肘关节前馈",
    agent: "claude",
    resident: "claude-t0ur",
    machine: "laptop",
    model: "Opus 5.5",
    town: "mark50",
    here: "desktop",
    events: s.recent(),
  };
}
