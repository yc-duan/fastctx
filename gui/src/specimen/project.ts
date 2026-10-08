// A project to look at its pages with: the map of mark50 and one of its
// journals, in the shapes the node answers `project map` and `journal read`.

import type { EntryStamp, JournalView, MapView, ProjectRecord, Writer } from "../world/world";
import { specimenPages } from "./pages";

const at = (time: string) => `2026-10-${time}Z`;
const claude: Writer = { kind: "resident", address: { machine: "laptop", resident: "claude-2b1c" } };
const codex: Writer = { kind: "resident", address: { machine: "laptop", resident: "codex-91c0" } };

const record: ProjectRecord = {
  version: 3,
  name: "mark50 研发",
  what: "为 Mark 50 战衣研发纳米单元的控制固件，并在真机上验证。",
  machines: ["desktop", "laptop", "lab"],
  places: [
    { what: "固件源码（git 仓库）", machine: "laptop", path: "/home/me/mark50" },
    { what: "定论", machine: "laptop", path: "/home/me/mark50/specs" },
    { what: "仿真与训练数据", machine: "lab", path: "/data/mark50-sim" },
    { what: "持续集成", machine: "lab", path: "/srv/mark50-ci" },
  ],
  canon: { machine: "laptop", path: "/home/me/mark50/specs" },
  by: { kind: "user" },
  on: "desktop",
  at: at("01T09:12:00"),
};

function stamp(id: string, time: string, by: Writer, agent: "claude" | "codex", extra: Partial<EntryStamp> = {}): EntryStamp {
  return {
    id,
    at: at(time),
    machine: "laptop",
    by,
    agent: agent === "claude" ? { agent: "claude", client: "claude-code", version: "2.1.289" } : { agent: "codex", client: "codex-mcp-client", version: "0.160.0" },
    model: agent === "claude" ? { name: "claude-opus-5-5", self_reported: false } : { name: "gpt-6.1-sol", self_reported: false },
    session: null,
    cites: [],
    attachments: [],
    ...extra,
  };
}

export function specimenMap(): MapView {
  return {
    town: "mark50",
    ledger: "/home/me/.fastctx/world/towns/mark50/ledger",
    record,
    opened: { by: { kind: "user" }, on: "desktop", at: at("01T09:12:00") },
    pages: specimenPages(),
    journals: [
      {
        journal: "q16-fixed-point",
        description: { version: 1, text: "Fixed-point arithmetic in the joint controllers: precision, saturation and where it was measured.", by: claude, on: "laptop", at: at("04T13:10:00") },
        entries: 3,
        writers: 2,
        latest: stamp("20261004T133400Z-laptop", "04T13:34:00", codex, "codex"),
      },
      {
        journal: "joint-model-training",
        description: { version: 2, text: "Training the joint model in simulation on lab: runs, settings and what each produced.", by: codex, on: "lab", at: at("03T22:00:00") },
        entries: 9,
        writers: 3,
        latest: stamp("20261004T070500Z-lab", "04T07:05:00", codex, "codex", { machine: "lab" }),
      },
      {
        journal: "webrtc-weak-network",
        description: { version: 1, text: "WebRTC push streaming over weak and lossy links: loss, freezes, and settings tried.", by: { kind: "user" }, on: "desktop", at: at("02T08:00:00") },
        entries: 7,
        writers: 3,
        latest: stamp("20261002T142011Z-desktop", "02T14:20:11", claude, "claude", { machine: "desktop" }),
      },
    ],
    canon: {
      Ok: [
        { file: "control-loop.md", title: "控制环", scope: "关节控制环的结构、定点数的取舍，以及在真机上验收的条件。" },
        { file: "telemetry.md", title: "遥测", scope: "纳米单元上报什么、多久一次、经哪条链路回到地面站。" },
        { file: "safety.md", title: "安全边界", scope: "任何时候都不许越过的力矩、温度与电流上限，以及越过时怎样停。" },
      ],
    },
  };
}

export function specimenJournal(): JournalView {
  return {
    town: "mark50",
    journal: "q16-fixed-point",
    directory: "/home/me/.fastctx/world/towns/mark50/ledger/journals/q16-fixed-point",
    description: { version: 1, text: "Fixed-point arithmetic in the joint controllers: precision, saturation and where it was measured.", by: claude, on: "laptop", at: at("04T13:10:00") },
    first: 1,
    total: 3,
    writers: 2,
    entries: [
      {
        stamp: stamp("20261004T131000Z-laptop", "04T13:10:00", claude, "claude", {
          attachments: [{ name: "step-response-q16.csv", kind: "text", bytes: 48213, sha256: "", kept: { kind: "in_entry" } }],
        }),
        text: "To see why `fixed::tests::step_response` settled at 0.9612 on lab (ctrl 0.4.1, commit 3f2a9c1), measured the integral's rounding at Ki = 0.8, dt = 1 ms.\n\nWith the integral accumulated in Q16, each step's `error * dt` below 2^-16 is lost; the steady-state error predicted from that, 2^-16 / (Ki dt) ≈ 0.019, matches the measured 0.0388 gap within a factor of two. With the integral in Q32 and narrowed after multiplying by Ki, the same test settled at 0.9999 (42 passed, 3m12s on lab).",
        mentioned_by: ["20261004T133400Z-laptop"],
      },
      {
        stamp: stamp("20261004T131800Z-laptop", "04T13:18:00", claude, "claude"),
        text: "Asked colleague codex-91c0 to find inputs where the Q32 integral or its narrowing wraps instead of saturating. Open question: whether `narrow()` saturates at all; its body is a plain shift and cast (`src/ctrl/q16.rs@3f2a9c1`, lines 88–90).",
        mentioned_by: [],
      },
      {
        stamp: stamp("20261004T133400Z-laptop", "04T13:34:00", codex, "codex", {
          cites: ["20261004T131000Z-laptop"],
          attachments: [{ name: "overflow_repro.rs", kind: "text", bytes: 1820, sha256: "", kept: { kind: "in_entry" } }],
        }),
        text: "With `ki = 0x7FFF_0000` and a constant error of 32767 at `dt = 1`, the Q32 integral saturated after 65 538 steps as meant, and `narrow()` then wrapped the output from 32767 to −32768 (branch `colleague/q16-edge` on laptop, test `overflow_repro`, attached). Before the Q32 change the same input wrapped at step 2.",
        mentioned_by: [],
      },
    ],
  };
}
