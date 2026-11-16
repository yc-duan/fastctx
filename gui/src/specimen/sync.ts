// How the user's devices stand in syncing, in the specimen: desktop (this
// computer) caught up all day; laptop catching up after an afternoon away,
// with conversations, job output and files left; the phone offline these
// three hours, and what it will catch up on return. With ?sync=all every
// device is caught up; with ?sync=many the account has some hundred and
// thirty devices, most caught up, a few catching up, a score offline and
// two that never reported; with ?sync=cut this computer is cut off from the
// hub.

import type { HubId } from "../world/accounts";
import type { DeviceSync, LagPoint, SyncStatusReply } from "../world/sync";

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
  const asked = new URLSearchParams(location.search).get("sync");
  const all = asked === "all";
  if (asked === "cut") return { local: { machine: "desktop", unsent_frames: 41, unsent_bytes: 2_310_000, cut_off: now - 12 * MINUTE }, unreached: "the hub is not reached" };
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
  const many = asked === "many" ? crowd(now, hours, step, wave) : { devices: [], points: [] };
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
        ...many.devices,
      ],
      points: [...points, ...many.points],
      totals: { conversations: 486, files: 2_311, file_bytes: 8_700_000_000, record_bytes: 1_260_000_000 },
    },
  };
}

/** A crowd of devices, as a school's or a lab's account has: rooms of
 *  computers, most caught up, a few catching up, a score switched off for
 *  the night and two on a version that does not report. */
function crowd(now: number, hours: number, step: number, wave: (ago: number) => number): { devices: DeviceSync[]; points: LagPoint[] } {
  const devices: DeviceSync[] = [];
  const points: LagPoint[] = [];
  const rooms = ["lab-a", "lab-b", "lab-c", "studio", "office"];
  let index = 0;
  for (const room of rooms) {
    for (let seat = 1; seat <= 26; seat += 1) {
      index += 1;
      const machine = `${room}${String(seat).padStart(2, "0")}`;
      const quiet = { unpulled: 0, missing_objects: 0, missing_bytes: 0, unsent_frames: 0, unsent_bytes: 0, rate: 0 };
      if (index % 61 === 7) {
        devices.push({ machine, kind: "computer", online: true, since: now - 4 * HOUR, behind: 0, ...quiet, synced: false });
        continue;
      }
      const off = index % 6 === 0;
      const busy = !off && index % 37 === 5;
      const gone = (8 + (index % 9)) * HOUR;
      points.push(
        ...line(machine, now, hours, step, (ago) => {
          if (off && ago < gone) return null;
          if (busy && ago < 50 * MINUTE) return Math.round(35 * MINUTE * (ago / (50 * MINUTE)) + 4 * MINUTE);
          return wave(ago + index * 7 * MINUTE);
        }),
      );
      if (off) {
        const left = index % 12 === 0 ? ([["conversations", 600_000 * (index % 7)], ["other", 90_000]] as DeviceSync["left"]) : undefined;
        devices.push({ machine, kind: "computer", online: false, since: now - gone, reported: now - gone, behind: gone, ...(left ? { left } : {}), ...quiet, synced: !left });
      } else if (busy) {
        devices.push({ machine, kind: "computer", online: true, since: now - 50 * MINUTE, reported: now - 12_000, behind: 4 * MINUTE + index * 20_000, left: [["conversations", 9_000_000 * (index % 5 + 1)], ["output", 2_400_000]], about: true, unpulled: index % 3, missing_objects: index % 4, missing_bytes: (index % 4) * 3_100_000, unsent_frames: 0, unsent_bytes: 0, rate: 2_800_000, synced: false });
      } else {
        devices.push({ machine, kind: "computer", online: true, since: now - (20 + (index % 30)) * HOUR, reported: now - 25_000, behind: 0, ...quiet, synced: true });
      }
    }
  }
  return { devices, points };
}
