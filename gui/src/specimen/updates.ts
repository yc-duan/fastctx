// New versions in the specimen, for a plain browser: this device runs a test
// release, GitHub lists newer ones, a test and a stable one with packages
// and one whose package for this device is not there, and the package of
// the one picked comes in a few seconds, through the network when the
// specimen says GitHub cannot be reached. Other machines answer about their
// own updates. `?update=none` offers nothing; `?update=ready` has the
// package here already; `?update=offline` cannot reach GitHub and hears of
// releases from the network; `?update=major` lists a release past the hub's
// major version.

import { compareVersions, type Manifest, type Release, type UpdateCall, type UpdateSeam, type UpdateSettings, type UpdateState } from "../world/updates";
import { runningOn, SPECIMEN_VERSION } from "./versions";
import type { RepairReport, RepairStep } from "../world/repair";

const TAKES = 6_000;
const VERSION = SPECIMEN_VERSION;

/** The releases GitHub lists, the newest first, and the size of each one's
 *  packages, absent where a release has none for a device. */
const RELEASES: { version: string; day: string; size: number | null }[] = [
  { version: "1.1.0-beta.1", day: "2026-11-05T09:00:00Z", size: 15_100_000 },
  { version: "1.0.1", day: "2026-10-30T09:00:00Z", size: 14_600_000 },
  { version: "1.0.1-beta.4", day: "2026-10-22T09:00:00Z", size: null },
  { version: VERSION, day: "2026-10-17T09:00:00Z", size: 14_200_000 },
  { version: "1.0.1-beta.2", day: "2026-10-16T06:33:00Z", size: 14_100_000 },
  { version: "1.0.1-beta.1", day: "2026-10-15T13:26:00Z", size: 14_000_000 },
];
const MAJOR = { version: "2.0.0-beta.1", day: "2026-11-20T09:00:00Z", size: 16_000_000 };

/** Other machines, by name: what each runs and is. */
const MACHINES: Record<string, { running: string; app?: string; auto: boolean; relays: boolean }> = {
  server: { running: runningOn("server"), auto: true, relays: true },
  gpu: { running: runningOn("gpu"), auto: false, relays: true },
  studio: { running: runningOn("studio"), app: runningOn("studio"), auto: false, relays: true },
};

function manifestOf(version: string, size: number | null, target: string, triple: string): Manifest {
  const pack = { signature: "", ...(size ? { size } : {}) };
  return {
    version,
    notes: "Other machines are updated from here, and an update kept for a machine away is made once it is back.",
    platforms: size ? { [target]: pack } : {},
    ...(size ? { fastctx: { [triple]: { signature: "", size: Math.round(size * 0.6) } } } : {}),
  };
}

export function specimenUpdates(params: URLSearchParams, phone: boolean): { seam: UpdateSeam; version: string } {
  const target = phone ? "android-apk" : "windows-x86_64-nsis";
  const triple = "x86_64-unknown-linux-gnu";
  const offline = params.get("update") === "offline";
  const listed = params.get("update") === "major" ? [MAJOR, ...RELEASES] : RELEASES;
  const releases: Release[] = listed.map((one) => ({ version: one.version, published: one.day, prerelease: one.version.includes("-") }));
  const after = (running: string) => listed.filter((one) => compareVersions(one.version, running) > 0);
  const newer = (running: string) => after(running).filter((one) => one.size).map((one) => manifestOf(one.version, one.size, target, triple));
  const newest = (running: string) => newer(running).find((manifest) => !manifest.version.startsWith("2."));
  let settings: UpdateSettings = { auto: false, relays: !phone };
  let chosen: Manifest | null = null;
  let started: number | null = params.get("update") === "ready" ? 0 : null;
  // `?repair=failed`: the update here did not go through, and a repair is
  // offered; `?repair=broken`, the repair itself stops short.
  const repairing = params.get("repair");
  let repairStarted: number | null = null;
  const repairReport = (): RepairReport | undefined => {
    if (repairStarted === null) return undefined;
    const layers = ["package", "records", "program", "service", "application", "node"];
    const broken = repairing === "broken";
    const stop = broken ? layers.indexOf("service") + 1 : layers.length;
    const done = Math.floor((Date.now() - repairStarted) / 900);
    const steps: RepairStep[] = layers.slice(0, Math.min(stop, done + 1)).map((layer, at) => {
      const outcome = at >= done ? "checking" : layer === "program" ? "repaired" : broken && layer === "service" ? "failed" : "unchanged";
      return { layer, outcome, detail: outcome === "repaired" ? "replaced the damaged program" : outcome === "failed" ? "the service would not start" : "", actions: [] };
    });
    const finished = done >= stop;
    return {
      id: `repair-${repairStarted}`,
      version: VERSION,
      finished: finished ? new Date().toISOString() : null,
      steps,
      success: finished && !broken,
      unchanged: false,
      error: finished && broken ? "service: systemctl start exited with 1" : null,
    };
  };
  const network = (): import("../world/updates").UpdateNetwork => ({
    ...(newest(VERSION) ? { beta: newest(VERSION)! } : {}),
    policy: { channel: "beta" as const, minimum: "1.0.1-beta.2", managed_auto: true },
    machines: Object.fromEntries(Object.entries(MACHINES).map(([name, one]) => [name, { kernel: one.running, ...(one.app ? { app: one.app } : {}) }])),
    hub: VERSION,
  });
  const state = (): UpdateState => {
    const offer = params.get("update") === "none" ? undefined : (chosen ?? newest(VERSION));
    const coming = started !== null && Date.now() - started < TAKES;
    const here = started !== null && !coming;
    const size = offer?.platforms[target]?.size ?? 0;
    return {
      running: VERSION,
      settings,
      channel: "beta",
      auto: settings.chosen ? settings.auto : false,
      hub_major: 1,
      looked: offline
        ? { at: new Date().toISOString(), github: false, why: "GitHub did not answer" }
        : { at: new Date(Date.now() - 4 * 60_000).toISOString(), github: true, releases, newer: newer(VERSION) },
      network: network(),
      ...(offer ? { offer } : {}),
      ...(chosen ? { chosen } : {}),
      ...(coming && offer
        ? { fetching: { version: offer.version, target, via: offline ? "network" : "github", received: Math.floor(((Date.now() - started!) / TAKES) * size), total: size } }
        : {}),
      kept: here && offer ? [{ version: offer.version, target, path: "" }] : [],
      app: { version: VERSION, target },
      kernel_target: "",
      repair_supported: true,
      ...(repairing && offer && repairStarted === null ? { failed: { version: offer.version, target, why: "拒绝访问。 (os error 5)" } } : {}),
      ...(() => {
        const report = repairReport();
        if (!report) return {};
        const now = report.steps[report.steps.length - 1];
        return { repair: report, ...(report.finished === null && now ? { stage: { kind: "repairing" as const, version: VERSION, layer: now.layer } } : {}) };
      })(),
    };
  };
  /** How another machine's updates stand, as its node would answer. */
  const machines = new Map<string, { settings: UpdateSettings; asked: string | null; at: number }>();
  const remote = (name: string): UpdateState => {
    const one = MACHINES[name] ?? { running: VERSION, auto: false, relays: true };
    const running = one.app ?? one.running;
    // `renewal=admin`: lab was updated and stopped for an
    // administrator there.
    const stopped = params.get("renewal") === "admin" && name === "lab" ? (newest(running)?.version ?? null) : null;
    const kept = machines.get(name) ?? { settings: { auto: one.auto, chosen: true, relays: one.relays }, asked: stopped, at: 0 };
    machines.set(name, kept);
    const asked = kept.asked ? newer(running).find((manifest) => manifest.version === kept.asked) : undefined;
    const elapsed = Date.now() - kept.at;
    const app = one.app ? { version: one.app, target: "windows-x86_64-nsis" } : undefined;
    const offer = asked ?? newest(running);
    return {
      running: one.running,
      settings: kept.settings,
      channel: running.includes("-") ? "beta" : "stable",
      auto: kept.settings.auto,
      hub_major: 1,
      looked: { at: new Date(Date.now() - 50 * 60_000).toISOString(), github: name !== "gpu", releases, newer: newer(running) },
      network: network(),
      ...(offer ? { offer } : {}),
      ...(asked && elapsed < TAKES
        ? { fetching: { version: asked.version, target: app ? app.target : `fastctx-${triple}`, via: "network", received: Math.floor((elapsed / TAKES) * 8_500_000), total: 8_500_000 } }
        : {}),
      ...(asked && elapsed >= TAKES ? { renewing: { version: asked.version, phase: stopped ? "updating_guardian" : "waiting_for_idle" } } : {}),
      ...(app ? { app, open: false, ...(kept.asked ? { take: kept.asked } : {}) } : {}),
      kernel_target: `fastctx-${triple}`,
    };
  };
  const seam: UpdateSeam = {
    state: async () => state(),
    progress: async () => state(),
    look: async () => {
      await new Promise((done) => setTimeout(done, 600));
      return state();
    },
    set: async (next) => {
      settings = next;
      return state();
    },
    fetch: async () => {
      started ??= Date.now();
      return state();
    },
    choose: async (version) => {
      chosen = version ? (newer(VERSION).find((manifest) => manifest.version === version) ?? null) : null;
      started = null;
      return state();
    },
    install: async () => {
      await new Promise((done) => setTimeout(done, 1_500));
      return true;
    },
    repair: async () => {
      await new Promise((done) => setTimeout(done, 300));
      repairStarted = Date.now();
      return state();
    },
    machine: async (name) => remote(name),
    remote: async (name, call: UpdateCall) => {
      await new Promise((done) => setTimeout(done, 250));
      const kept = machines.get(name) ?? (remote(name), machines.get(name)!);
      if (call.do === "set") kept.settings = call.settings;
      if (call.do === "kernel") {
        kept.asked = call.version ?? newest(MACHINES[name]?.app ?? MACHINES[name]?.running ?? VERSION)?.version ?? null;
        kept.at = Date.now();
      }
      return remote(name);
    },
  };
  return { seam, version: VERSION };
}
