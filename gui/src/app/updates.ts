// What the interface knows of new versions of itself (`world-upgrade.md`,
// 界面上的样子): how they stand, asked of this computer's node or this
// phone's device every minute, every second while a package comes; whether
// a version newer than this one is offered for this device; and, with
// automatic updates on, bringing it here at once and installing it once
// nothing here is under way.

import { signal, type Signal } from "./signals";
import { compareVersions, majorOf, type FailureReason, type Manifest, type UpdateSeam, type UpdateSettings, type UpdateStage, type UpdateState } from "../world/updates";

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

export class Updates {
  readonly state: Signal<UpdateState | null> = signal<UpdateState | null>(null);
  readonly offered: Signal<Offered | null> = signal<Offered | null>(null, (a, b) => JSON.stringify(a) === JSON.stringify(b));
  private installing = false;
  private permission = false;
  private failed: string | null = null;
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

  /** Brings the offered version here, or installs it when it is here. */
  go(): void {
    const offered = this.offered.peek();
    if (!offered || this.installing) return;
    this.failed = null;
    this.permission = false;
    if (offered.stage.kind === "ready" || offered.stage.kind === "waiting" || offered.stage.kind === "permission") {
      this.install(offered.manifest.version);
      return;
    }
    void this.seam.fetch(offered.manifest.version).then(
      (state) => this.take(state),
      (error: unknown) => {
        this.failed = String(error);
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
        this.failed = String(error);
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
    this.refresh();
    this.later(state.fetching ? 1_000 : this.offered.peek()?.stage.kind === "waiting" ? 5_000 : 60_000);
  }

  /** What is offered now, and what automatic updates do with it: taken by
   *  itself, a release never passes the hubs' major version; one asked
   *  from elsewhere is taken as an automatic update is. */
  private refresh(): void {
    const state = this.state.peek();
    const offered = state ? this.offerOf(state) : null;
    this.offered.set(offered);
    if (state) this.tell(state, offered);
    if (!state || !offered || this.installing || this.failed) return;
    const asked = state.take === offered.manifest.version;
    // The node says which release automatic updates may take; a node before
    // it said so is reckoned with here, under the hubs' major version.
    const within =
      state.automatic !== undefined
        ? state.automatic === offered.manifest.version
        : state.hub_major === undefined || majorOf(offered.manifest.version) <= state.hub_major;
    if (!asked && !(this.automatic(state) && within)) return;
    if (offered.stage.kind === "offered") this.go();
    else if (offered.stage.kind === "waiting" && this.idle()) this.install(offered.manifest.version);
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
    if (this.failed) return { ...base, stage: { kind: "failed", why: this.failed } };
    const fetching = state.fetching;
    if (fetching && fetching.version === manifest.version && fetching.target === target) {
      return {
        ...base,
        stage: { kind: "fetching", received: fetching.received, total: fetching.total ?? pack.size ?? null, network: fetching.via === "network" },
      };
    }
    if (state.kept?.some((kept) => kept.version === manifest.version && kept.target === target)) {
      if (this.permission) return { ...base, stage: { kind: "permission" } };
      const waits = this.automatic(state) || state.take === manifest.version;
      return { ...base, stage: { kind: waits ? "waiting" : "ready" } };
    }
    const failed = state.failed;
    if (failed && failed.version === manifest.version && failed.target === target) {
      return { ...base, stage: { kind: "failed", why: failed.why, ...(failed.reason ? { reason: failed.reason } : {}) } };
    }
    return { ...base, stage: { kind: "offered" } };
  }
}
