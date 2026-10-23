// What the interface knows of new versions of itself (`world-upgrade.md`,
// 界面上的样子): how they stand, asked of this computer's node or this
// phone's device every minute, every second while a package comes; whether
// a version newer than this one is offered for this device; and, with
// automatic updates on, bringing it here at once and installing it once
// nothing here is under way.

import { signal, type Signal } from "./signals";
import { compareVersions, majorOf, type FailureReason, type Manifest, type UpdateSeam, type UpdateSettings, type UpdateStage, type UpdateState } from "../world/updates";
import type { RepairReport } from "../world/repair";

/** What the sidebar's button and the card show. */
export interface Offered {
  manifest: Manifest;
  /** The package's size, when the release says it. */
  size: number | null;
  stage:
    | { kind: "offered" }
    | { kind: "fetching"; received: number; total: number | null; network: boolean }
    | { kind: "ready" }
    /** With automatic updates on: ready, waiting for what is under way. */
    | { kind: "waiting" }
    | { kind: "installing" }
    /** A phone sent the user to allow installing apps. */
    | { kind: "permission" }
    | { kind: "failed"; why: string; reason?: FailureReason };
  /** The network asks for at least this version, which this one is not. */
  minimum: string | null;
}

/** A repair started from this interface, followed until it ends: the
 *  record the device kept before it, to tell its own from that one; its
 *  newest record since; what kept it from starting. */
export interface Repairing {
  before: string | null;
  report: RepairReport | null;
  error: string | null;
}

/** Whether this device is stuck where a repair is what helps: an update
 *  that did not go through, a World program change stopped short or
 *  waiting on its startup service, or the World program here and the
 *  interface on different versions. */
export function troubled(state: UpdateState | null, offered: Offered | null): boolean {
  if (!state) return false;
  if (offered?.stage.kind === "failed" || state.failed || state.renewing?.error) return true;
  const stage = state.stage?.kind;
  if (stage === "failed" || stage === "administrator") return true;
  return !!(state.running && state.app && state.running !== state.app.version);
}

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

export class Updates {
  readonly state: Signal<UpdateState | null> = signal<UpdateState | null>(null);
  readonly offered: Signal<Offered | null> = signal<Offered | null>(null, (a, b) => JSON.stringify(a) === JSON.stringify(b));
  readonly repairing: Signal<Repairing | null> = signal<Repairing | null>(null, (a, b) => JSON.stringify(a) === JSON.stringify(b));
  private installing = false;
  private permission = false;
  /** What failed here, of which release: bringing it, or installing it. */
  private failed: { version: string; step: "fetch" | "install"; why: string } | null = null;
  /** A failed release tried again by itself: which, since when, how often. */
  private retried = { version: "", at: 0, count: 0 };
  private timer: ReturnType<typeof setTimeout> | null = null;
  private known: string | null = null;
  /** What was last told the node of what only the interface knows. */
  private told = "idle";

  constructor(
    private readonly seam: UpdateSeam,
    /** Whether nothing here is under way: no agent in a turn, nobody typing. */
    private readonly idle: () => boolean,
  ) {
    this.ask();
  }

  /** This application's version, once the shell said it. */
  get version(): string {
    return this.known ?? "";
  }

  /** The shell says which version this application is. */
  know(version: string): void {
    this.known = version;
    this.refresh();
  }

  /** Looks on GitHub now. */
  look(): Promise<void> {
    return this.seam.look().then((state) => this.take(state));
  }

  set(settings: UpdateSettings): Promise<void> {
    return this.seam.set(settings).then((state) => this.take(state));
  }

  /** Updates this device to `version`, newer than it and not only the
   *  newest: it is offered from now on, and brought here as the newest
   *  would be (`world-upgrade.md`, 版本窗). */
  pick(version: string): Promise<void> {
    return this.seam.choose(version).then((state) => {
      this.take(state);
      this.go();
    });
  }

  /** Repairs this device layer by layer, and follows the repair until it
   *  ends; a repair under way already is followed, not begun again. */
  repair(): void {
    const going = this.repairing.peek();
    if (going && !going.error && going.report?.finished == null) return;
    const before = this.state.peek()?.repair?.id ?? null;
    this.repairing.set({ before, report: null, error: null });
    this.seam.repair().then(
      (state) => this.take(state),
      (error: unknown) => this.repairing.set({ before, report: null, error: String(error) }),
    );
  }

  /** The repair's end has been seen: the card shows how things stand. */
  forgetRepair(): void {
    this.repairing.set(null);
  }

  /** Brings the offered version here, or installs it when it is here. */
  go(): void {
    const offered = this.offered.peek();
    if (!offered || this.installing) return;
    const version = offered.manifest.version;
    // What failed installing here is here already: it is installed again.
    const installed = this.failed?.version === version && this.failed.step === "install";
    this.failed = null;
    this.permission = false;
    if (installed || offered.stage.kind === "ready" || offered.stage.kind === "waiting" || offered.stage.kind === "permission") {
      this.install(version);
      return;
    }
    void this.seam.fetch(version).then(
      (state) => this.take(state),
      (error: unknown) => {
        this.failed = { version, step: "fetch", why: String(error) };
        this.refresh();
      },
    );
  }

  private install(version: string): void {
    this.installing = true;
    this.refresh();
    this.seam.install(version).then(
      (started) => {
        // A computer starts again as the new version; a phone hands it to
        // the system, which asks the user and replaces this application.
        this.installing = false;
        this.permission = !started;
        this.refresh();
      },
      (error: unknown) => {
        this.installing = false;
        // The shell told the node, which keeps it, and when to try it again,
        // past this window.
        this.failed = { version, step: "install", why: String(error) };
        this.refresh();
      },
    );
  }

  private ask(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.seam.state().then(
      (state) => this.take(state),
      () => this.later(60_000),
    );
  }

  private later(after: number): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => this.ask(), after);
  }

  private take(state: UpdateState): void {
    this.state.set(state);
    const going = this.repairing.peek();
    if (going && state.repair && state.repair.id !== going.before) this.repairing.set({ ...going, report: state.repair });
    this.refresh();
    const following = this.repairing.peek();
    const repairing = following !== null && !following.error && following.report?.finished == null;
    this.later(state.fetching || repairing ? 1_000 : this.offered.peek()?.stage.kind === "waiting" ? 5_000 : 60_000);
  }

  /** What is offered now, and what automatic updates do with it: taken by
   *  itself, a release never passes the hubs' major version; one asked
   *  from elsewhere is taken as an automatic update is. */
  private refresh(): void {
    const state = this.state.peek();
    const offered = state ? this.offerOf(state) : null;
    this.offered.set(offered);
    if (state) this.tell(state, offered);
    if (!state || !offered || this.installing) return;
    const asked = state.take === offered.manifest.version || this.unfinished(state, offered.manifest.version);
    // The node says which release automatic updates may take; a node before
    // it said so is reckoned with here, under the hubs' major version.
    const within =
      state.automatic !== undefined
        ? state.automatic === offered.manifest.version
        : state.hub_major === undefined || majorOf(offered.manifest.version) <= state.hub_major;
    if (!asked && !(this.automatic(state) && within)) return;
    if (offered.stage.kind === "offered") this.go();
    else if (offered.stage.kind === "waiting" && this.idle()) this.install(offered.manifest.version);
    else if (offered.stage.kind === "failed" && this.due(state, offered.manifest.version)) {
      this.retried = { version: offered.manifest.version, at: Date.now(), count: this.retried.count + 1 };
      // What failed here is asked again as it now stands: a package kept
      // here is installed, not brought again.
      if (this.failed) {
        this.failed = null;
        this.refresh();
      } else this.go();
    }
  }

  /** Whether a failed release is tried again by itself now: when its node
   *  says, or else an hour after it failed, then two, four, up to a day,
   *  so a machine nobody tends is never left behind by one failure. */
  private due(state: UpdateState, version: string): boolean {
    const now = Date.now();
    if (this.retried.version !== version) this.retried = { version, at: now, count: 0 };
    // Never more than once an hour from here, whatever the node says.
    const since = this.retried.count ? now - this.retried.at : Infinity;
    if (this.failed?.version !== version && state.retry_at) return Date.parse(state.retry_at) <= now && since >= HOUR;
    return now >= this.retried.at + Math.min(HOUR * 2 ** this.retried.count, DAY);
  }

  /** Tells the node that an update waits here for the work under way, or
   *  that it no longer does; a node that takes no such word is not told. */
  private tell(state: UpdateState, offered: Offered | null): void {
    if (!state.progress) return;
    const stage: UpdateStage = offered?.stage.kind === "waiting" ? { kind: "waiting_idle", version: offered.manifest.version } : { kind: "idle" };
    const said = JSON.stringify(stage);
    if (said === this.told) return;
    this.told = said;
    this.seam.progress(stage).catch(() => {
      this.told = "";
    });
  }

  /** Whether `version` was begun here and failed: updating by itself
   *  decides whether a release is begun, not whether one that failed is
   *  finished, which is tried again on its schedule and installed once
   *  nothing is under way (`world-upgrade.md`, 没成的，自己再试). */
  private unfinished(state: UpdateState, version: string): boolean {
    const target = state.app?.target;
    const failed = state.failed;
    const refused = failed?.reason?.kind === "no_package" || failed?.reason?.kind === "not_newer";
    return this.failed?.version === version || (!!failed && !refused && failed.version === version && failed.target === target);
  }

  /** Whether this device takes new releases by itself, as its node says. */
  private automatic(state: UpdateState): boolean {
    return state.auto ?? state.settings.auto;
  }

  private offerOf(state: UpdateState): Offered | null {
    const manifest = state.available ?? state.offer;
    const target = state.app?.target;
    if (!manifest || !target || !this.known || compareVersions(manifest.version, this.known) <= 0) return null;
    const pack = manifest.platforms[target];
    if (!pack) return null;
    const minimum = state.network?.policy.minimum;
    const below = minimum && compareVersions(this.version, minimum) < 0 ? minimum : null;
    const base = { manifest, size: pack.size ?? null, minimum: below };
    if (this.installing) return { ...base, stage: { kind: "installing" } };
    // A failure here is of its release alone: a newer one is offered afresh.
    if (this.failed?.version === manifest.version) return { ...base, stage: { kind: "failed", why: this.failed.why } };
    const fetching = state.fetching;
    if (fetching && fetching.version === manifest.version && fetching.target === target) {
      return {
        ...base,
        stage: { kind: "fetching", received: fetching.received, total: fetching.total ?? pack.size ?? null, network: fetching.via === "network" },
      };
    }
    // One that failed here waits for its time to be tried again, its
    // package kept or not.
    const failed = state.failed;
    const ofThis = failed && failed.version === manifest.version && failed.target === target;
    if (ofThis && state.retry_at && Date.parse(state.retry_at) > Date.now()) {
      return { ...base, stage: { kind: "failed", why: failed.why, ...(failed.reason ? { reason: failed.reason } : {}) } };
    }
    if (state.kept?.some((kept) => kept.version === manifest.version && kept.target === target)) {
      if (this.permission) return { ...base, stage: { kind: "permission" } };
      const waits = this.automatic(state) || state.take === manifest.version || this.unfinished(state, manifest.version);
      return { ...base, stage: { kind: waits ? "waiting" : "ready" } };
    }
    if (ofThis) return { ...base, stage: { kind: "failed", why: failed.why, ...(failed.reason ? { reason: failed.reason } : {}) } };
    return { ...base, stage: { kind: "offered" } };
  }
}
