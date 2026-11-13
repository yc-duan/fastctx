// How the user's devices stand in syncing (`gui-sync-status-page`, design
// §28.9): drawn more than written. At the top, how many of them are caught
// up; then a paper card for each device — caught up, catching up with what
// is left by kind, or offline for how long with what it will catch up on
// return; the line of how far behind each stood over the last day or week,
// a line each in its own ink, the offline stretches blank; what the hub
// holds of the account; and what this computer has not sent, which it
// knows even when the hub cannot be reached. The numbers are the hub's,
// asked again each minute while the page is open.

import { bytesSaid } from "../bytes";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { loginsOf, type HubId, type Login } from "../world/accounts";
import type { DeviceSync, Part, SyncStatusReply } from "../world/sync";
import { redrawKeepingFocus } from "./focus";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import type { Store } from "./store";
import { CAUGHT_UP, INKS, lagLines } from "./sync-draw";
import type { SyncWords } from "./sync-words";
import { fitInks, parts } from "./usage/draw";

export interface SyncPageOptions {
  store: Store;
  words: SyncWords;
  head(title: string, actions?: string): string;
  /** How `account`'s devices stand on `hub` over the last `hours`. */
  read(hub: HubId, account: string, hours: number): Promise<SyncStatusReply>;
  now(): number;
}

type Span = "day" | "week";
const SPAN_KEY = "sync.span";
const HOURS: Record<Span, number> = { day: 24, week: 24 * 7 };
/** How often the page asks again while it is open. */
const AGAIN_MS = 60_000;
/** The inks of what is left, by kind. */
const PART_INKS: Record<Part | "files", string> = {
  conversations: "var(--pen-clay)",
  output: "var(--pen-sky)",
  other: "var(--pen-sage)",
  files: "var(--pen-rose)",
};

function keptSpan(): Span {
  try {
    return localStorage.getItem(SPAN_KEY) === "week" ? "week" : "day";
  } catch {
    return "day";
  }
}

type Read = { reply: SyncStatusReply } | { problem: string };

export class SyncPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly spans: HTMLElement;
  private span: Span = keptSpan();
  private readonly read = new Map<string, Read>();
  private timer: number | null = null;
  private readonly resize: ResizeObserver;
  private width = 0;
  /** The width the line is drawn at: the column's, as last measured. */
  private chartWidth = 0;

  constructor(private readonly options: SyncPageOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page sync-page";
    const switcher = `<div class="segmented sync-spans" role="radiogroup" aria-label="${escape(words.spanLabel)}"></div>`;
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`, switcher)}<div class="settings sync-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".sync-body")!;
    this.spans = this.element.querySelector<HTMLElement>(".sync-spans")!;
    this.spans.addEventListener("click", (event) => {
      const span = (event.target as HTMLElement).closest<HTMLElement>("[data-span]")?.dataset.span as Span | undefined;
      if (!span || span === this.span) return;
      this.span = span;
      try {
        localStorage.setItem(SPAN_KEY, span);
      } catch {
        // Kept for this visit only.
      }
      this.drawSpans();
      void this.ask();
    });
    this.drawSpans();
    this.resize = new ResizeObserver(() => {
      const width = Math.round(this.body.clientWidth);
      if (width === this.width) return;
      this.width = width;
      this.draw();
    });
    this.resize.observe(this.body);
    this.draw();
    void this.ask();
    this.timer = window.setInterval(() => void this.ask(), AGAIN_MS);
  }

  leave(): void {
    if (this.timer !== null) window.clearInterval(this.timer);
    this.timer = null;
    this.resize.disconnect();
  }

  private logins(): Login[] {
    return loginsOf(this.options.store.world.peek()?.accounts);
  }

  private drawSpans(): void {
    const { words } = this.options;
    this.spans.innerHTML = (["day", "week"] as const)
      .map((span) => `<button type="button" class="segment${span === this.span ? " chosen" : ""}" role="radio" aria-checked="${span === this.span}" data-span="${span}">${escape(words.spans[span])}</button>`)
      .join("");
  }

  /** Asks how each login's devices stand, all at once. */
  private async ask(): Promise<void> {
    const hours = HOURS[this.span];
    await Promise.all(
      this.logins().map(async (login) => {
        const key = `${login.hub}\n${login.account}`;
        try {
          this.read.set(key, { reply: await this.options.read(login.hub, login.account, hours) });
        } catch (error) {
          this.read.set(key, { problem: detailOf(error) });
        }
      }),
    );
    this.draw();
  }

  private draw(): void {
    const { words } = this.options;
    const logins = this.logins();
    const width = Math.max(240, this.chartWidth || 640);
    const html = logins.length
      ? logins
          .map((login) => {
            const read = this.read.get(`${login.hub}\n${login.account}`);
            const named = logins.length > 1 ? `<h2 class="sync-login-name">${escape(login.account)} · ${escape(this.hubName(login.hub))}</h2>` : "";
            const inside = !read
              ? `<p class="feature-note">${escape(words.reading)}</p>`
              : "problem" in read
                ? `<p class="feature-note" title="${escape(read.problem)}">${escape(words.failed)}</p>`
                : this.login(read.reply, width, login);
            return `<section class="sync-login">${named}${inside}</section>`;
          })
          .join("")
      : `<p class="feature-note">${escape(words.noneReported)}</p>`;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
    fitInks(this.body);
    // The line is drawn at the column's width: measured once drawn, and
    // drawn again where it was another.
    const column = this.body.querySelector<HTMLElement>(".sync-line");
    const measured = column ? Math.round(column.clientWidth) : 0;
    if (measured > 0 && Math.abs(measured - this.chartWidth) > 2) {
      this.chartWidth = measured;
      this.draw();
    }
  }

  private hubName(hub: HubId): string {
    return this.options.store.world.peek()?.accounts.hubs.find((one) => one.hub === hub)?.name ?? hub;
  }

  /** One login's part: the summary, the cards, the line, the hub's totals,
   *  and this computer's own. */
  private login(reply: SyncStatusReply, width: number, login: Login): string {
    const { words } = this.options;
    const status = reply.hub;
    const here = reply.local;
    const now = this.options.now();
    const ownCard = `<section class="sync-here">
      <h3 class="sync-heading">${escape(words.hereTitle)}</h3>
      ${here.unread
        ? `<p class="sync-fact" title="${escape(here.unread)}">${escape(words.failed)}</p>`
        : `<p class="sync-fact">${escape(here.unsent_frames ? words.unsent(here.unsent_frames, bytesSaid(here.unsent_bytes)) : words.nothingUnsent)}</p>`}
      ${here.cut_off ? `<p class="sync-fact caution">${escape(words.cutOff(words.duration(now - here.cut_off)))}</p>` : ""}
    </section>`;
    if (!status) {
      return `<p class="feature-note"${reply.unreached ? ` title="${escape(reply.unreached)}"` : ""}>${escape(words.unreached)}</p>${ownCard}`;
    }
    // Every device of the account counts, one that never reported among those
    // not caught up: "all caught up" is said of every card below (I05).
    const devices = status.devices;
    const synced = devices.filter((device) => device.synced).length;
    const summary = !devices.length ? words.noneReported : synced === devices.length ? words.allSynced(devices.length) : words.someSynced(synced, devices.length);
    const inks = new Map(status.devices.map((device, index) => [device.machine, INKS[Math.min(index, INKS.length - 1)]!]));
    const cards = status.devices.map((device) => this.card(device, inks.get(device.machine)!, device.machine === here.machine, now)).join("");
    const span = this.span;
    const lines = status.devices
      .filter((device) => status.points.some((point) => point.machine === device.machine))
      .map((device) => ({ machine: device.machine, name: device.machine, ink: inks.get(device.machine)!, points: status.points.filter((point) => point.machine === device.machine) }));
    const chart = lines.length
      ? lagLines({ width, height: 200, from: status.at - status.hours * 3_600_000, to: status.at, step: status.step, span, lines, words, seed: `${login.hub}:${login.account}` })
      : `<p class="feature-note">${escape(words.lineEmpty)}</p>`;
    const legend = lines.length > 1 ? `<ul class="sync-legend">${lines.map((line) => `<li><span class="sync-key" style="--ink:${line.ink}" aria-hidden="true"></span>${escape(line.name)}</li>`).join("")}</ul>` : "";
    const totals = status.totals;
    return `<section class="sync-top">
        <p class="sync-summary${synced === devices.length && devices.length ? " synced" : ""}">${escape(summary)}</p>
      </section>
      <section class="sync-cards">${cards}</section>
      <section class="sync-line">
        <h3 class="sync-heading" title="${escape(words.lineSaid)}">${escape(words.lineTitle)}</h3>
        ${legend}
        ${chart}
      </section>
      <section class="sync-hub">
        <h3 class="sync-heading" title="${escape(words.hubSaid)}">${escape(words.hubTitle)}</h3>
        <p class="sync-totals"><span>${escape(words.conversations(totals.conversations))}</span><span>${escape(words.hubFiles(totals.files, bytesSaid(totals.file_bytes)))}</span><span>${escape(words.records(bytesSaid(totals.record_bytes)))}</span></p>
      </section>
      ${ownCard}`;
  }

  /** A device's paper card. */
  private card(device: DeviceSync, ink: string, thisOne: boolean, now: number): string {
    const { words } = this.options;
    const kind = device.kind === "computer" ? "computer" : device.kind === "phone" ? "phone" : "other";
    const left = (device.left ?? []).reduce((sum, [, bytes]) => sum + bytes, 0);
    const leftSaid = left ? `${device.about ? "≈ " : ""}${bytesSaid(left)}` : null;
    let state: string;
    let tone = "";
    if (device.reported === undefined) {
      state = words.neverReported;
      tone = " quiet";
    } else if (!device.online) {
      // Nothing left is said only where nothing is known to be left: no
      // record, no file, no scope not yet pulled, nothing only about so (I04).
      const since = words.duration(Math.max(0, now - (device.since ?? device.reported)));
      const unknown = !!device.about || device.unpulled > 0;
      state = leftSaid
        ? words.away(since, leftSaid)
        : unknown
          ? words.awayUnknown(since)
          : device.missing_objects > 0
            ? words.awayFiles(since, device.missing_objects)
            : words.away(since, null);
      tone = " away";
    } else if (device.synced) {
      state = words.synced;
      tone = " synced";
    } else if (!left && device.behind < CAUGHT_UP && !device.unpulled && device.missing_objects) {
      state = words.textSynced(device.missing_objects);
    } else {
      const eta = left && device.rate ? ` · ${words.eta(words.duration((left / device.rate) * 1000))}` : "";
      state = leftSaid ? `${words.catching(leftSaid)}${eta}` : words.behindBy(words.duration(device.behind));
    }
    const shares = [
      ...(device.left ?? []).map(([part, bytes]) => ({ value: bytes, ink: PART_INKS[part], said: `${words.parts[part]} ${bytesSaid(bytes)}` })),
      ...(device.missing_bytes ? [{ value: device.missing_bytes, ink: PART_INKS.files, said: `${words.filesPart} ${bytesSaid(device.missing_bytes)}` }] : []),
    ].filter((share) => share.value > 0);
    const bar = shares.length ? parts(shares, `sync:${device.machine}`, 320) : "";
    const keys = shares.length > 1 ? `<ul class="sync-parts">${shares.map((share) => `<li><span class="sync-swatch" style="--ink:${share.ink}" aria-hidden="true"></span>${escape(share.said)}</li>`).join("")}</ul>` : "";
    const facts = [
      device.unpulled ? words.unpulled(device.unpulled) : "",
      device.missing_objects && !(device.synced) ? words.files(device.missing_objects, bytesSaid(device.missing_bytes)) : "",
      device.unsent_frames ? words.unsent(device.unsent_frames, bytesSaid(device.unsent_bytes)) : "",
    ].filter(Boolean);
    return `<article class="sync-card${tone}">
      <header class="sync-card-head">
        <span class="sync-card-ink" style="--ink:${ink}" aria-hidden="true"></span>
        ${glyph(kind === "phone" ? "phone" : "desktop", 16)}
        <b class="sync-card-name">${escape(device.machine)}</b>
        ${thisOne ? `<span class="sync-tag">${escape(words.thisOne)}</span>` : ""}
        <span class="sync-card-kind">${escape(words.kinds[kind])}</span>
      </header>
      <p class="sync-state">${escape(state)}</p>
      ${bar}
      ${keys}
      ${facts.length ? `<p class="sync-card-facts">${facts.map((fact) => `<span>${escape(fact)}</span>`).join("")}</p>` : ""}
    </article>`;
  }
}
