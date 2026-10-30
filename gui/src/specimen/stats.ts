// How the specimen's machines say they are doing, for a machine's page in a
// plain browser: the readings drift with the clock as a real machine's do,
// so the drawings move; the lines to the hub carry a month of comings and
// goings. studio is offline; gpu runs a fastctx too old to be asked.

import { seeded } from "../pen/shapes";
import type { Card, Disk, Gpu, LimitReading, LimitUse, LineChange, MachineStats, Trip } from "../world/stats";
import { runningOn } from "./versions";

const GB = 1024 ** 3;
const DAY = 86_400_000;

interface Profile {
  system: string;
  kernel?: string;
  host: string;
  cpu: { model: string; threads: number; load?: boolean; base: number; swing: number };
  memory: { total: number; used: number; swing: number; swap: number; swapUsed: number; waiting?: number };
  disks: Disk[];
  cards: { name: string; hardware: boolean; up: boolean; speed?: number; down: number; up_: number; addresses: string[] }[];
  gpus?: { name: string; memory: number; used: number; busy: number }[];
  hubs: { hub: string; through?: string; ms: number }[];
  /** Days since it started, and hours since its node did. */
  booted: number;
  started: number;
  service: boolean;
  /** Its nights off the network: a laptop asleep. */
  sleeps?: boolean;
  /** How much of each of its system's limits it held a day ago and holds
   *  now, against the limit. */
  limits?: { kind: LimitUse["kind"]; from: number; to: number; limit: number }[];
}

const windows = (host: string): Pick<Profile, "system" | "host"> => ({ system: "Windows 11 Pro 24H2 (26200.6584)", host });

const profiles: Record<string, Profile> = {
  desktop: {
    ...windows("DESKTOP-EXAMPLE"),
    cpu: { model: "AMD Ryzen 9 7950X 16-Core Processor", threads: 32, base: 0.16, swing: 0.12 },
    memory: { total: 64 * GB, used: 27.5 * GB, swing: 2.2 * GB, swap: 72 * GB, swapUsed: 38 * GB },
    disks: [
      { at: "C:/", label: "系统", fs: "NTFS", total: 1000 * GB, free: 212 * GB },
      { at: "D:/", label: "work", fs: "NTFS", total: 2000 * GB, free: 1210 * GB },
      { at: "E:/", fs: "NTFS", total: 4000 * GB, free: 132 * GB },
    ],
    cards: [
      { name: "Ethernet", hardware: true, up: true, speed: 1e9, down: 1.4e6, up_: 90e3, addresses: ["192.168.1.20", "fd00::20"] },
      { name: "Meta", hardware: false, up: true, down: 0.6e6, up_: 40e3, addresses: ["198.18.0.1"] },
      { name: "Wi-Fi", hardware: true, up: false, down: 0, up_: 0, addresses: [] },
    ],
    gpus: [{ name: "NVIDIA GeForce RTX 4090", memory: 24 * GB, used: 6.2 * GB, busy: 0.35 }],
    hubs: [{ hub: "home", ms: 41 }],
    booted: 3.3,
    started: 51,
    service: true,
  },
  laptop: {
    ...windows("LAPTOP-EXAMPLE"),
    cpu: { model: "Intel(R) Core(TM) Ultra 7 155H", threads: 22, base: 0.09, swing: 0.07 },
    memory: { total: 32 * GB, used: 17.8 * GB, swing: 1.1 * GB, swap: 38 * GB, swapUsed: 21 * GB },
    disks: [{ at: "C:/", fs: "NTFS", total: 1000 * GB, free: 388 * GB }],
    cards: [{ name: "Wi-Fi", hardware: true, up: true, speed: 866.7e6, down: 220e3, up_: 30e3, addresses: ["192.168.1.41"] }],
    hubs: [{ hub: "home", ms: 46 }],
    booted: 6.1,
    started: 2.6,
    service: true,
    sleeps: true,
  },
  lab: {
    system: "Ubuntu 24.04.1 LTS",
    kernel: "Linux 6.8.0-45-generic",
    host: "lab",
    cpu: { model: "Intel(R) Xeon(R) Gold 6338 CPU @ 2.00GHz", threads: 16, load: true, base: 0.42, swing: 0.3 },
    memory: { total: 128 * GB, used: 71 * GB, swing: 6 * GB, swap: 8 * GB, swapUsed: 0.4 * GB },
    disks: [
      { at: "/", fs: "ext4", total: 480 * GB, free: 302 * GB },
      { at: "/data", label: "data", fs: "xfs", total: 8000 * GB, free: 690 * GB },
    ],
    cards: [{ name: "eno1", hardware: true, up: true, speed: 10e9, down: 9.5e6, up_: 2.4e6, addresses: ["10.0.0.5", "192.168.1.30"] }],
    hubs: [{ hub: "home", through: "server", ms: 12 }],
    booted: 41,
    started: 210,
    service: true,
  },
  server: {
    system: "Ubuntu 24.04.1 LTS",
    kernel: "Linux 6.8.0-45-generic",
    host: "server",
    cpu: { model: "", threads: 2, load: true, base: 0.06, swing: 0.05 },
    memory: { total: 3.9 * GB, used: 2.62 * GB, swing: 0.12 * GB, swap: 6 * GB, swapUsed: 1.6 * GB, waiting: 2 },
    disks: [{ at: "/", fs: "ext4", total: 39 * GB, free: 21.4 * GB }],
    cards: [{ name: "eth0", hardware: true, up: true, down: 310e3, up_: 290e3, addresses: ["203.0.113.20"] }],
    hubs: [{ hub: "home", ms: 3 }],
    booted: 74,
    started: 300,
    service: true,
    limits: [
      { kind: "inotify_instances", from: 31, to: 118, limit: 128 },
      { kind: "inotify_watches", from: 4_100, to: 9_211, limit: 524_288 },
      { kind: "open_files", from: 120, to: 412, limit: 65_536 },
    ],
  },
  build: {
    system: "Debian GNU/Linux 12 (bookworm)",
    kernel: "Linux 6.1.0-25-amd64",
    host: "build",
    cpu: { model: "AMD EPYC 7763 64-Core Processor", threads: 64, load: true, base: 0.55, swing: 0.35 },
    memory: { total: 256 * GB, used: 120 * GB, swing: 18 * GB, swap: 0, swapUsed: 0 },
    disks: [{ at: "/", fs: "ext4", total: 1900 * GB, free: 640 * GB }],
    cards: [{ name: "ens5", hardware: true, up: true, speed: 25e9, down: 22e6, up_: 4e6, addresses: ["198.51.100.40"] }],
    hubs: [{ hub: "acme", ms: 22 }],
    booted: 12,
    started: 100,
    service: true,
  },
};

/** A smooth wander through time: the same moment always reads the same,
 *  and readings two seconds apart are close. */
function drift(seed: string, t: number): number {
  const random = seeded(seed);
  const waves = [0, 1, 2].map(() => ({ period: 40_000 + random() * 260_000, phase: random() * Math.PI * 2, weight: 0.3 + random() * 0.7 }));
  const total = waves.reduce((sum, wave) => sum + wave.weight, 0);
  return waves.reduce((sum, wave) => sum + wave.weight * Math.sin((t / wave.period) * Math.PI * 2 + wave.phase), 0) / total;
}

/** Noise for one moment, from a seed and the two-second step it falls in. */
function jitter(seed: string, t: number): number {
  return seeded(`${seed}:${Math.floor(t / 2000)}`)() - 0.5;
}

const clamp = (value: number) => Math.max(0, Math.min(1, value));

/** A month of a line's comings and goings, oldest first: the node's
 *  starts, short drops with the reason the node gave, and a laptop's
 *  nights asleep. */
function changesOf(name: string, profile: Profile, now: number): LineChange[] {
  const random = seeded(`changes:${name}`);
  const changes: LineChange[] = [];
  const iso = (at: number) => new Date(at).toISOString();
  const reasons = ["nothing came from the hub for 30 s", "the network changed", "the hub closed the connection", "the machine woke from sleep"];
  const started = now - profile.started * 3_600_000;
  let at = Math.max(now - 30 * DAY, now - profile.booted * DAY);
  changes.push({ kind: "started", at: iso(at) }, { kind: "up", at: iso(at + 1500) });
  at += 1500;
  while (at < now) {
    const step = profile.sleeps ? DAY * (0.4 + random() * 0.6) : DAY * (1 + random() * 4);
    at += step;
    if (at >= started && at - step < started) {
      changes.push({ kind: "started", at: iso(started) }, { kind: "up", at: iso(started + 1800) });
      at = started + 1800;
      continue;
    }
    if (at >= now - 120_000) break;
    const nightly = profile.sleeps && random() < 0.8;
    const away = nightly ? 3_600_000 * (5 + random() * 4) : 20_000 + random() * 140_000;
    const why = nightly ? reasons[3]! : reasons[Math.floor(random() * 3)]!;
    changes.push({ kind: "down", at: iso(at), why });
    if (at + away >= now - 60_000) break;
    changes.push({ kind: "up", at: iso(at + away) });
    at += away;
  }
  return changes;
}

/** An hour of round trips, one each heartbeat (ten seconds). */
function tripsOf(name: string, base: number, now: number): Trip[] {
  const trips: Trip[] = [];
  const last = Math.floor(now / 10_000) * 10_000;
  for (let at = last - 3_600_000 + 10_000; at <= last; at += 10_000) {
    const spike = seeded(`spike:${name}:${at}`)() < 0.025 ? 60 + seeded(`spike-size:${name}:${at}`)() * 160 : 0;
    const ms = Math.max(1, Math.round(base * (1 + 0.12 * drift(`rtt:${name}`, at)) + jitter(`rtt:${name}`, at) * base * 0.3 + spike));
    trips.push({ at: new Date(at).toISOString(), ms });
  }
  return trips;
}

/** What `machine` (this computer, desktop, with none) says of itself now;
 *  with `after`, its lines bring only what came since. */
export async function specimenStats(machine: string | null, after: string | null = null): Promise<MachineStats> {
  const name = machine ?? "desktop";
  await new Promise((resolve) => setTimeout(resolve, 120 + Math.random() * 160));
  if (name === "studio") throw new Error("studio is offline: it was last online 2026-10-04T12:52:00Z");
  const profile = profiles[name];
  if (!profile) throw new Error(`${name} answered something other than how it is doing`);
  const now = Date.now();
  const iso = (at: number) => new Date(at).toISOString();
  const since = after ? Date.parse(after) : -Infinity;
  const busy = clamp(profile.cpu.base + profile.cpu.swing * drift(`cpu:${name}`, now) + jitter(`cpu:${name}`, now) * 0.06);
  const used = profile.memory.used + profile.memory.swing * drift(`memory:${name}`, now);
  const cards: Card[] = profile.cards.map((card) => {
    const surge = Math.max(0, drift(`net:${name}:${card.name}`, now));
    const down = card.up ? Math.max(0, card.down * (0.25 + 1.6 * surge) * (1 + jitter(`down:${name}:${card.name}`, now))) : undefined;
    const sent = card.up ? Math.max(0, card.up_ * (0.3 + 1.4 * surge) * (1 + jitter(`up:${name}:${card.name}`, now))) : undefined;
    return {
      name: card.name,
      up: card.up,
      ...(card.hardware ? { hardware: true } : {}),
      ...(card.speed ? { speed: card.speed } : {}),
      received: card.up ? Math.round(card.down * 3600 * 24 * profile.booted * 0.4) : 0,
      sent: card.up ? Math.round(card.up_ * 3600 * 24 * profile.booted * 0.4) : 0,
      ...(down !== undefined ? { receiving: down } : {}),
      ...(sent !== undefined ? { sending: sent } : {}),
      ...(card.addresses.length ? { addresses: card.addresses } : {}),
    };
  });
  const gpus: Gpu[] = (profile.gpus ?? []).map((gpu) => ({
    name: gpu.name,
    memory_total: gpu.memory,
    memory_used: Math.round(gpu.used * (1 + 0.15 * drift(`vram:${name}`, now))),
    busy: clamp(gpu.busy + 0.3 * drift(`gpu:${name}`, now) + jitter(`gpu:${name}`, now) * 0.08),
  }));
  const load = (minutes: number) => Math.max(0, profile.cpu.threads * (profile.cpu.base + profile.cpu.swing * drift(`cpu:${name}`, now - minutes * 30_000)));
  return {
    machine: name,
    at: iso(now),
    boot: iso(now - profile.booted * DAY),
    system: profile.system,
    ...(profile.kernel ? { kernel: profile.kernel } : {}),
    host: profile.host,
    arch: "x86_64",
    cpu: {
      model: profile.cpu.model,
      threads: profile.cpu.threads,
      busy,
      ...(profile.cpu.load ? { load: [load(1), load(5), load(15)].map((value) => Math.round(value * 100) / 100) as [number, number, number] } : {}),
    },
    memory: {
      total: profile.memory.total,
      used: Math.round(used),
      swap_total: profile.memory.swap,
      swap_used: profile.memory.swapUsed,
      room: 1.5 * GB,
      ...(profile.memory.waiting ? { waiting: profile.memory.waiting } : {}),
    },
    disks: profile.disks,
    cards,
    ...(gpus.length ? { gpus } : {}),
    lines: profile.hubs.map((hub) => {
      const changes = changesOf(name, profile, now);
      const last = changes[changes.length - 1]!;
      const up = last.kind === "up";
      return {
        hub: hub.hub,
        since: last.at,
        up,
        ...(hub.through ? { through: hub.through } : {}),
        trips: up ? tripsOf(name, hub.ms, now).filter((trip) => Date.parse(trip.at) > since) : [],
        changes: changes.filter((change) => Date.parse(change.at) > since),
      };
    }),
    node: {
      version: runningOn(name),
      started: iso(now - profile.started * 3_600_000),
      service: profile.service,
      // `?refused=1`: macOS kept a folder of this computer's from its node.
      ...(name === "desktop" && new URLSearchParams(location.search).has("refused") ? { refused: { path: "/Users/me/Desktop", at: iso(now - 60_000) } } : {}),

      // `?ledger=1`: its usage ledger set rows aside.
      ...(name === "desktop" && new URLSearchParams(location.search).has("ledger")
        ? { health: [{ kind: "warning", text: "usage ledger /home/me/.fastctx/world/usage/claude-1 has 3 unreadable rows preserved in quarantine; totals omit these rows" }] }
        : {}),
      // `?stuck=1`: some of its own work has gone on too long.
      ...(name === "desktop" && new URLSearchParams(location.search).has("stuck")
        ? { stuck: [{ doing: "task 128037", what: "holds draining(this machine)", seconds: 4_740 }] }
        : {}),
    },
    // Who runs how many jobs there: the lab is shared with ada, up to four.
    ...(name === "lab"
      ? {
          jobs: [
            { account: { hub: "w-0123456789abcdef", account: "ada" }, running: 3, most: 4 },
            { running: 2 },
            { account: { hub: "w-0123456789abcdef", account: "bo" }, running: 1 },
          ],
        }
      : {}),
    ...(profile.limits ? { limits: limitsOf(name, profile.limits, now, since) } : {}),
  };
}

/** A day of readings of a machine's limits, one a minute after `since`:
 *  each climbing from what it held a day ago, in steps as conversations
 *  came and went. */
function limitsOf(name: string, limits: NonNullable<Profile["limits"]>, now: number, since: number): LimitReading[] {
  const minute = 60_000;
  const end = Math.floor(now / minute) * minute;
  const readings: LimitReading[] = [];
  for (let at = end - DAY + minute; at <= end; at += minute) {
    if (at <= since) continue;
    const share = (at - (end - DAY)) / DAY;
    readings.push({
      at: new Date(at).toISOString(),
      uses: limits.map(({ kind, from, to, limit }) => {
        const step = Math.floor(share * 18) / 18;
        const wobble = (seeded(`${name}:${kind}:${Math.floor(at / (40 * minute))}`)() - 0.5) * (to - from) * 0.08;
        return { kind, used: Math.max(0, Math.round(from + (to - from) * step + wobble)), limit };
      }),
    });
  }
  return readings;
}
