// A window shares one wait for named state. Each reader has its own in-flight
// read and retry: a slow machine cannot delay the other pages. Timed readers
// use a separate clock and leave the shared wait open.
import { Channel, invoke } from "@tauri-apps/api/core";
import { inShell, unwatch } from "./bridge";
import { Backoff } from "./backoff";

export const LOGINS = "logins";
export const SENTINELS = "sentinels";
export const UPDATE = "update";
export const BOOK = "book";
export const mapTopic = (town: string): string => `map/${town}`;

type Read = () => void | Promise<unknown>;
type Every = number | (() => number | false);
interface At { topic: string; revision: number }
interface Told { run: string; changed: At[] }
type Answer = { kind: "told"; told: Told } | { kind: "unsupported" } | { kind: "aborted" } | { kind: "error"; error: string };
class WaitAborted extends Error {}
interface Reader {
  name: () => string | null;
  topic: string | null;
  read: Read;
  every: Every;
  due: number;
  fresh: boolean;
  reading: boolean;
  again: boolean;
  notBefore: number;
}

// Longer than the node's 25-second wait, including connection/setup time.
const WAIT_MS = 35_000;

function asking(run: string | null, known: At[], signal: AbortSignal): Promise<Told | null> {
  return new Promise((resolve, reject) => {
    let done = false;
    let cancelled = false;
    let timer = 0;
    const finish = (answer?: Answer, error?: unknown) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      signal.removeEventListener("abort", abort);
      if (error !== undefined) reject(error);
      else if (!answer) reject(new DOMException("cancelled", "AbortError"));
      else if (answer.kind === "aborted") reject(new WaitAborted("the state's wait was aborted"));
      else if (answer.kind === "error") reject(new Error(answer.error));
      else resolve(answer.kind === "told" ? answer.told : null);
    };
    const abort = () => {
      cancelled = true;
      void opened.then(unwatch).catch(() => {}).finally(() => finish());
    };
    const answer = new Channel<Answer>();
    answer.onmessage = (reply) => { if (!cancelled) finish(reply); };
    const opened = invoke<number>("world_topics", { request: { run, known }, answer });
    timer = window.setTimeout(() => {
      cancelled = true;
      void opened.then(unwatch).catch(() => {});
      finish(undefined, new Error("the state's wait did not answer in time"));
    }, WAIT_MS);
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) abort();
    void opened.catch((error: unknown) => finish(undefined, error ?? new Error("the state's wait could not be opened")));
  });
}

/** Unsubscribes; refresh re-evaluates a conditional timer after a manual read. */
export interface Following { (): void; refresh(): void }

/** Unknown names and readers with no topic keep their visible timed reads. */
export function followState(topic: string | null | (() => string | null), read: Read, every: Every): Following {
  return topics.follow(topic, read, every);
}

/** Specimen pages use their own seams and must never open a node connection. */
export function configureTopics(enabled: boolean): void {
  topics.enable(enabled);
}

class Topics {
  private enabled = true;
  private readonly readers = new Set<Reader>();
  private readonly revisions = new Map<string, number>();
  private run: string | null = null;
  private supported: boolean | null = null;
  private negotiateAt = Infinity;
  private retryAt = 0;
  private readonly backoff = new Backoff();
  private queued = false;
  private readersChanged = false;
  private waiting: AbortController | null = null;
  private working = false;
  private timer = 0;

  constructor() {
    document.addEventListener("visibilitychange", () => {
      this.waiting?.abort();
      if (!document.hidden) {
        this.supported = null;
        this.retryAt = 0;
        for (const reader of this.readers) { reader.fresh = true; reader.due = 0; }
      }
      this.tick();
    });
  }

  enable(enabled: boolean): void {
    this.enabled = enabled;
    this.waiting?.abort();
    this.tick();
  }

  follow(name: string | null | (() => string | null), read: Read, every: Every): Following {
    const reader: Reader = { name: typeof name === "function" ? name : () => name, topic: null, read, every, due: 0, fresh: true, reading: false, again: false, notBefore: 0 };
    this.readers.add(reader);
    this.readersChanged = true;
    this.schedule();
    const stop: Following = Object.assign(
      () => { this.readers.delete(reader); this.readersChanged = true; this.schedule(); },
      { refresh: () => {
        if (!this.readers.has(reader) || reader.reading) return;
        const interval = typeof every === "function" ? every() : every;
        reader.due = interval === false ? Infinity : Math.min(reader.due, Date.now() + interval);
        this.tick();
      } },
    );
    return stop;
  }

  private names(): string[] {
    let changed = false;
    for (const reader of this.readers) {
      const topic = this.enabled && inShell() ? reader.name() : null;
      if (topic !== reader.topic) { reader.topic = topic; reader.fresh = true; reader.due = 0; changed = true; }
    }
    if (changed) this.waiting?.abort();
    const names = [...new Set([...this.readers].flatMap((reader) => reader.topic ? [reader.topic] : []))];
    for (const name of this.revisions.keys()) if (!names.includes(name)) this.revisions.delete(name);
    return names;
  }

  private timed(reader: Reader): boolean {
    // Revision 0 means this node does not know this particular name.
    return !reader.topic || this.supported === false || this.revisions.get(reader.topic) === 0;
  }

  private read(reader: Reader): void {
    if (!this.readers.has(reader) || document.hidden) return;
    if (reader.reading) { reader.again = true; return; }
    // A walk or download can publish thousands of progress changes. Keep
    // the latest, including the final change before a writer stalls.
    if (!this.timed(reader) && Date.now() < reader.notBefore) {
      reader.fresh = true;
      reader.due = reader.notBefore;
      this.tick();
      return;
    }
    reader.notBefore = Date.now() + 100;
    reader.reading = true;
    reader.fresh = false;
    reader.due = Infinity;
    let failed = false;
    void Promise.resolve().then(reader.read).catch(() => { failed = true; }).finally(() => {
      reader.reading = false;
      const every = typeof reader.every === "function" ? reader.every() : reader.every;
      reader.fresh ||= failed;
      reader.due = Date.now() + (failed ? (every === false ? 1_000 : every) : every === false ? Infinity : every);
      if (reader.again) { reader.again = false; this.read(reader); }
      this.tick();
    });
  }

  private next(reader: Reader): number {
    return reader.reading || (!this.timed(reader) && !reader.fresh) ? Infinity : reader.due;
  }

  private schedule(): void {
    if (this.queued) return;
    this.queued = true;
    queueMicrotask(() => { this.queued = false; this.tick(); });
  }

  private tick(): void {
    clearTimeout(this.timer);
    if (this.readersChanged) { this.readersChanged = false; this.waiting?.abort(); }
    if (document.hidden || !this.readers.size) return;
    const names = this.names();
    const now = Date.now();
    if (this.supported === false && names.length && now >= this.negotiateAt) this.supported = null;
    for (const reader of this.readers) {
      if (this.next(reader) > now) continue;
      if (!this.timed(reader)) this.waiting?.abort();
      else this.read(reader);
    }
    this.start();
    // A named reader's initial read waits for the snapshot. Once the loop
    // starts, its pending snapshot needs no clock tick of its own.
    const deadlines = [...this.readers].map((reader) => !this.timed(reader) && (this.working || this.retryAt > now) && this.next(reader) <= now ? Infinity : this.next(reader));
    const next = Math.min(...deadlines, names.length ? (this.supported === false ? this.negotiateAt : this.retryAt > now ? this.retryAt : Infinity) : Infinity);
    if (Number.isFinite(next)) this.timer = window.setTimeout(() => this.tick(), Math.max(0, next - Date.now()));
  }

  private start(): void {
    if (this.working || document.hidden || !this.readers.size || this.supported === false || this.retryAt > Date.now() || !this.names().length) return;
    this.working = true;
    void this.loop().finally(() => { this.working = false; this.tick(); });
  }

  private async loop(): Promise<void> {
    while (!document.hidden && this.readers.size && this.supported !== false) {
      const names = this.names();
      if (!names.length) return;
      const readers = [...this.readers];
      const snapshot = names.some((name) => !this.revisions.has(name)) || readers.some((reader) => !this.timed(reader) && reader.fresh && !reader.reading && reader.due <= Date.now());
      const known = names.map((topic) => ({ topic, revision: this.revisions.get(topic) ?? 0 }));
      const controller = new AbortController();
      this.waiting = controller;
      try {
        const told = await asking(snapshot ? null : this.run, known, controller.signal);
        this.backoff.reset();
        this.retryAt = 0;
        if (told === null) {
          this.supported = false;
          this.negotiateAt = Date.now() + 60_000;
          return;
        }
        this.supported = true;
        const restarted = this.run !== told.run;
        this.run = told.run;
        const changed = new Set<string>();
        for (const at of told.changed) {
          if (restarted || this.revisions.get(at.topic) !== at.revision) changed.add(at.topic);
          this.revisions.set(at.topic, at.revision);
        }
        for (const reader of readers) if (reader.topic && (changed.has(reader.topic) || (reader.fresh && reader.due <= Date.now()))) this.read(reader);
      } catch (error) {
        if (!controller.signal.aborted) {
          if (error instanceof WaitAborted) {
            this.supported = null;
            this.run = null;
            this.revisions.clear();
            for (const reader of this.readers) { reader.fresh = true; reader.due = 0; }
          }
          this.retryAt = Date.now() + this.backoff.failed();
        }
        return;
      } finally {
        if (this.waiting === controller) this.waiting = null;
      }
    }
  }
}

const topics = new Topics();
