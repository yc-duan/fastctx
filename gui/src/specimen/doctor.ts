// What this computer's node says of how its Fastctx is doing, in the
// specimen: all well, and with ?health=strained held up, two things stuck,
// rules broken, a bound near its limit and scenes left.

import type { DoctorReadings } from "../world/doctor";

export async function specimenDoctor(): Promise<DoctorReadings> {
  if (new URLSearchParams(location.search).get("health") !== "strained") return { readings: {}, health: {}, scenes: [] };
  return {
    readings: {
      violations_total: 37,
      violations: [
        { kind: "blocking", place: "crates/fastctx-node/src/timers/store.rs:110", count: 30, doing: "timers", worst_ms: 48, last: "2026-10-04T13:31:00Z" },
        { kind: "slow-poll", place: "crates/fastctx-net/src/link/session.rs:612", count: 6, worst_ms: 212, last: "2026-10-04T12:58:00Z" },
        { kind: "held-up", place: "work", count: 1, worst_ms: 1240, last: "2026-10-04T13:38:00Z" },
      ],
    },
    health: { busy: true, held_up_ms: 1240, stuck: 2, violations: 37, near: { bound: "log.waiting", used: 92 } },
    scenes: ["C:/Users/me/.fastctx/scenes/scene-1759579860000.txt", "C:/Users/me/.fastctx/scenes/scene-1759581720000.txt", "C:/Users/me/.fastctx/scenes/scene-1759583880000.txt"],
  };
}
