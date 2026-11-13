// How the user's devices stand in syncing, in the specimen: desktop (this
// computer) caught up all day; laptop catching up after an afternoon away,
// with conversations, job output and files left; the phone offline these
// three hours, and what it will catch up on return. With ?sync=all every
// device is caught up.

import type { HubId } from "../world/accounts";
import type { LagPoint, SyncStatusReply } from "../world/sync";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

/** A line through the span, a point a step: `at(t)` says how far behind it
 *  stood `t` before now, and null where it was offline. */
function line(machine: string, now: number, hours: number, step: number, at: (ago: number) => number | null): LagPoint[] {
  const points: LagPoint[] = [];
  for (let t = now - hours * HOUR; t <= now; t += step) {
    const bucket = t - (t % step);
    const behind = at(now - bucket);
    points.push({ machine, at: bucket, behind: behind ?? 0, online: behind !== null });
  }
  return points;
}

export async function specimenSync(_hub: HubId, _account: string, hours: number): Promise<SyncStatusReply> {
  const now = Date.now();
  const step = hours <= 24 ? 5 * MINUTE : HOUR;
  const all = new URLSearchParams(location.search).get("sync") === "all";
  const wave = (ago: number) => Math.round(20_000 + 15_000 * Math.sin(ago / (37 * MINUTE)));
  const points = [
    ...line("desktop", now, hours, step, (ago) => (ago % (5 * HOUR) < 12 * MINUTE ? 90_000 + (ago % (12 * MINUTE)) * 6 : wave(ago))),
    ...line("laptop", now, hours, step, (ago) => {
      if (all) return wave(ago + HOUR);
      if (ago > 9 * HOUR && ago < 15 * HOUR) return null;
      if (ago <= 9 * HOUR && ago > 40 * MINUTE) return Math.round(6 * HOUR - (9 * HOUR - ago) * 0.62);
      if (ago <= 40 * MINUTE) return Math.round(42 * MINUTE * (ago / (40 * MINUTE)));
      return wave(ago);
    }),
    ...line("pixel", now, hours, step, (ago) => (!all && ago < 3 * HOUR ? null : ago > 20 * HOUR && ago < 22 * HOUR ? null : wave(ago + 2 * HOUR) + 15_000)),
  ];
  return {
    local: { machine: "desktop", unsent_frames: all ? 0 : 3, unsent_bytes: all ? 0 : 18_432 },
    hub: {
      at: now,
      hours,
      step,
      devices: [
        { machine: "desktop", kind: "computer", online: true, since: now - 26 * HOUR, reported: now - 20_000, behind: 0, unpulled: 0, missing_objects: 0, missing_bytes: 0, unsent_frames: all ? 0 : 3, unsent_bytes: all ? 0 : 18_432, rate: 0, synced: true },
        all
          ? { machine: "laptop", kind: "computer", online: true, since: now - 15 * HOUR, reported: now - 30_000, behind: 0, unpulled: 0, missing_objects: 0, missing_bytes: 0, unsent_frames: 0, unsent_bytes: 0, rate: 0, synced: true }
          : {
              machine: "laptop", kind: "computer", online: true, since: now - 9 * HOUR, reported: now - 8_000, behind: 42 * MINUTE,
              left: [["conversations", 48_300_000], ["output", 12_600_000], ["other", 2_100_000]], about: true,
              unpulled: 2, missing_objects: 14, missing_bytes: 96_000_000, unsent_frames: 0, unsent_bytes: 0, rate: 3_400_000, synced: false,
            },
        all
          ? { machine: "pixel", kind: "phone", online: true, since: now - 2 * HOUR, reported: now - 40_000, behind: 0, unpulled: 0, missing_objects: 0, missing_bytes: 0, unsent_frames: 0, unsent_bytes: 0, rate: 0, synced: true }
          : {
              machine: "pixel", kind: "phone", online: false, since: now - 3 * HOUR - 12 * MINUTE, reported: now - 3 * HOUR - 12 * MINUTE, behind: 3 * HOUR,
              left: [["conversations", 1_840_000], ["other", 220_000]], unpulled: 0, missing_objects: 3, missing_bytes: 4_200_000,
              unsent_frames: 0, unsent_bytes: 0, rate: 0, synced: false,
            },
      ],
      points,
      totals: { conversations: 486, files: 2_311, file_bytes: 8_700_000_000, record_bytes: 1_260_000_000 },
    },
  };
}
