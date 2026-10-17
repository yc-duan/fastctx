// An update of several machines at once (`world-upgrade.md`, 别的机器的更新).
// First the plan: which of them update to the version picked, which are away
// and kept by the hub for when they are back, which are skipped and why.
// Once started, a few at a time are asked to update and followed as each
// says how it goes; a machine whose package has come gives its turn to the
// next, so packages pass the hub a few at a time. A machine of a release
// before it answers about its updates updates to the newest on its line,
// followed by what the network hears it runs.

import type { Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { compareVersions, newestKnown, type UpdateCall, type UpdateState } from "../world/updates";
import type { BatchWords } from "./machine-batch-words";
import { updaterLoginOf } from "./machine-logins";
import type { Machine } from "./reach";

/** The first release that answers about its updates (`RemoteCall::Updates`). */
const ASKED_SINCE = "1.0.1-beta.3";
/** How many machines are asked at once, while their packages come. */
const AT_ONCE = 3;
/** How often a machine updating is asked how it goes. */
const EVERY_MS = 2_000;
/** How long a machine updating may not answer before it is said lost: it
 *  restarts on its new program meanwhile. */
const SILENT_MS = 180_000;

/** What becomes of a machine chosen, as planned. */
export type Fate =
  | { kind: "update"; login: Login; older: boolean; reserve: boolean }
  | { kind: "due"; login: Login }
  | { kind: "skip"; why: string };

/** How it goes once started. */
export type Going =
  | { kind: "queued" }
  | { kind: "asking" }
  | { kind: "state"; state: UpdateState; heard: number }
  | { kind: "older"; heard: number }
  | { kind: "done" }
  | { kind: "kept" }
  | { kind: "failed"; why: string };

export interface BatchRow {
  machine: Machine;
  /** What it ran when the plan was made. */
  running: string | null;
  fate: Fate;
  going: Going | null;
}

export interface BatchOptions {
  remote(machine: string, call: UpdateCall): Promise<UpdateState>;
  /** Has a machine of an earlier release update to the newest on its line. */
  older(machine: string): Promise<unknown>;
  /** Keeps an update for a machine away until it is back. */
  due(login: Login, machine: string, version: string | null, batch?: string): Promise<void>;
  /** What the network last heard a machine runs. */
  heard(machine: string): string | null;
  /** Something shown changed. */
  changed(): void;
}

/** What becomes of each of `machines` updated to `version`. */
export function planOf(machines: Machine[], world: WorldNow, version: string, local: UpdateState | null, words: BatchWords): BatchRow[] {
  return machines.map((machine): BatchRow => {
    const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    const running = local?.network?.machines?.[machine.name]?.kernel ?? facts?.version ?? null;
    const row = (fate: Fate): BatchRow => ({ machine, running, fate, going: null });
    if (machine.here) return row({ kind: "skip", why: words.skipHere });
    const login = updaterLoginOf(machine, world);
    if (!login) return row({ kind: "skip", why: words.skipNotYours });
    if (running && compareVersions(running, version) >= 0) return row({ kind: "skip", why: words.skipTaken });
    const older = !!running && compareVersions(running, ASKED_SINCE) < 0;
    // An update kept for a machine away is its own to make once back, from
    // 1.0.1 beta 3 on, and only a hub that reads such records keeps one.
    if (machine.online === false) {
      if (older) return row({ kind: "skip", why: words.skipAwayOlder });
      if (!world.accounts.hubs.find((hub) => hub.hub === login.hub)?.due) return row({ kind: "skip", why: words.skipHubOlder });
      return row({ kind: "due", login });
    }
    if (older && local?.network?.machines?.[machine.name]?.app) return row({ kind: "skip", why: words.skipOlderApp });
    if (older && newestKnown(local, running!) !== version) return row({ kind: "skip", why: words.skipOlder });
    return row({ kind: "update", login, older, reserve: !!world.accounts.hubs.find((hub) => hub.hub === login.hub)?.package_progress });
  });
}

export class UpdateBatch {
  started = false;
  private timer = 0;
  private left = false;
  private readonly id = crypto.randomUUID();
  private readonly reserved = new Set<BatchRow>();

  constructor(
    readonly version: string,
    readonly rows: BatchRow[],
    private readonly options: BatchOptions,
  ) {}

  /** How many the plan updates now or keeps for later. */
  get reaching(): number {
    return this.rows.filter((row) => row.fate.kind !== "skip").length;
  }

  /** Whether every machine started is through, one way or another. */
  get finished(): boolean {
    return this.started && this.rows.every((row) => row.fate.kind === "skip" || (row.going && ["done", "kept", "failed"].includes(row.going.kind)));
  }

  get done(): number {
    return this.rows.filter((row) => row.going?.kind === "done" || row.going?.kind === "kept").length;
  }

  get failed(): number {
    return this.rows.filter((row) => row.going?.kind === "failed").length;
  }

  start(): void {
    if (this.started) return;
    this.started = true;
    void this.prepare();
  }

  private async prepare(): Promise<void> {
    await Promise.all(this.rows.map(async (row) => {
      if (row.fate.kind === "skip") return;
      const { login } = row.fate;
      row.going = { kind: "asking" };
      this.options.changed();
      try {
        if (row.fate.kind === "due") {
          await this.options.due(login, row.machine.name, this.version);
          row.going = { kind: "kept" };
        } else {
          if (row.fate.reserve) {
            this.reserved.add(row);
            await this.options.due(login, row.machine.name, this.version, this.id);
          }
          row.going = { kind: "queued" };
          if (this.left) await this.release(row);
        }
      } catch (error) {
        row.going = { kind: "failed", why: said(error) };
        await this.release(row);
      }
    }));
    if (this.left) return;
    this.pump();
    this.follow();
    this.options.changed();
  }

  dispose(): void {
    this.left = true;
    clearTimeout(this.timer);
    for (const row of this.reserved) {
      if (row.going?.kind === "queued") void this.release(row);
    }
  }

  private async release(row: BatchRow): Promise<void> {
    if (!this.reserved.has(row) || row.fate.kind !== "update") return;
    try {
      await this.options.due(row.fate.login, row.machine.name, null, this.id);
      this.reserved.delete(row);
    } catch (error) {
      row.going = { kind: "failed", why: said(error) };
      this.options.changed();
    }
  }

  /** Asks the next machines in turn while fewer than a few are taking
   *  their packages. */
  private pump(): void {
    const taking = this.rows.filter((row) => row.going?.kind === "asking" || (row.going?.kind === "state" && !!row.going.state.fetching)).length;
    let room = AT_ONCE - taking;
    for (const row of this.rows) {
      if (room <= 0) break;
      if (row.going?.kind !== "queued" || row.fate.kind !== "update") continue;
      room -= 1;
      void this.ask(row, row.fate.older);
    }
  }

  private async ask(row: BatchRow, older: boolean): Promise<void> {
    row.going = { kind: "asking" };
    this.options.changed();
    try {
      if (older) {
        await this.options.older(row.machine.name);
        row.going = { kind: "older", heard: Date.now() };
      } else {
        const state = await this.options.remote(row.machine.name, { do: "kernel", version: this.version });
        row.going = this.read(state) ?? { kind: "state", state, heard: Date.now() };
      }
    } catch (error) {
      row.going = { kind: "failed", why: said(error) };
    }
    if (row.going?.kind === "done" || row.going?.kind === "failed") await this.release(row);
    this.after();
  }

  /** Done or failed, as a machine's answer says; nothing while it goes on. */
  private read(state: UpdateState): Going | null {
    if (state.running && compareVersions(state.running, this.version) >= 0) return { kind: "done" };
    if (state.renewing?.phase === "failed") return { kind: "failed", why: state.renewing.error ?? "" };
    if (state.failed && compareVersions(state.failed.version, this.version) === 0 && !state.fetching && !state.renewing) return { kind: "failed", why: state.failed.why };
    return null;
  }

  private after(): void {
    if (this.left) return;
    this.pump();
    this.options.changed();
  }

  /** Asks each machine under way how it goes, every few seconds, until all
   *  are through. */
  private follow(): void {
    clearTimeout(this.timer);
    if (this.left || this.finished) return;
    this.timer = window.setTimeout(() => {
      void Promise.all(this.rows.map((row) => this.check(row))).then(() => {
        this.after();
        this.follow();
      });
    }, EVERY_MS);
  }

  private async check(row: BatchRow): Promise<void> {
    const going = row.going;
    if (going?.kind === "older") {
      const heard = this.options.heard(row.machine.name);
      if (heard && compareVersions(heard, this.version) >= 0) { row.going = { kind: "done" }; await this.release(row); }
      return;
    }
    if (going?.kind !== "state") return;
    try {
      const state = await this.options.remote(row.machine.name, { do: "state" });
      row.going = this.read(state) ?? { kind: "state", state, heard: Date.now() };
      if (row.going.kind === "done" || row.going.kind === "failed") await this.release(row);
    } catch (error) {
      // Changing its program it restarts, and is not there to answer for
      // a while: quiet until long past that.
      if (Date.now() - going.heard > SILENT_MS) row.going = { kind: "failed", why: said(error) };
    }
  }
}

function said(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
