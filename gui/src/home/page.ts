// The World's front page: what the reader finds on coming back
// (`world-gui.md`, 回来时), in two views. Now: the drawing of the World as
// it is, each machine on it a way to its page, and under it what needs the
// reader, each conversation by what it asks, answered here where it offers
// its answers, or by what it told them that they have not seen
// (`world-voice.md`, 送到哪里). Lately: what came back while the reader was
// away, a line a conversation by what it last said, along a line of the
// time away, and beside it what is at work now, the conversations and the
// commands running. A part with nothing in it is not drawn. Every line is a
// fact the World holds; nothing is summarised by a model.

import type { VersionMismatch } from "../world/renewal";
import { fileLink } from "../app/places";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "../app/focus";
import type { Address } from "../world/events";
import type { Asked, ResidentNow, RunningJob } from "../world/world";
import { ownerResident } from "../world/world";
import { worksFor } from "../app/store";
import { blot, ruled, seeded } from "../pen/shapes";
import type { Happening, WorldState } from "./model";
import type { HomeWords } from "./words";
import { agentName, titleOf } from "../world/names";

export interface HomeOptions {
  words: HomeWords;
  /** When the reader last looked at this page, when known. */
  lastVisit: Date | null;
  /** Where a resident's conversation opens. */
  link(address: Address): string;
  /** The head every page has, with the button that brings back the sidebar. */
  head: string;
  /** Begins a conversation. */
  newTalk(): void;
  /** The drawing of the World, kept up by whoever made it. */
  drawing: HTMLElement;
  /** Stops a running command, on the machine it runs on, whoever started it. */
  stopJob?(job: string, machine: string): Promise<void>;
  /** Opens this device's updates, where the World's program here is put
   *  up to the interface's. */
  updates?(): void;
  /** The last thing a conversation (`resident@machine`) told that this
   *  device has shown the user. */
  seenTold(key: string): string | null;
}

type Glyph = Parameters<typeof glyph>[0];

const ASK_GLYPHS: Record<Asked["what"], Glyph> = {
  answer: "ask",
  command: "terminal",
  change: "edit",
  tool: "tool",
  plan: "tasks",
  form: "page",
  other: "ask",
};

/** What came back of one conversation while the reader was away: its last
 *  turn that ended, and how. */
interface Came {
  key: string;
  address: Address;
  at: Date;
  failed: boolean;
  said: string;
  r: ResidentNow | undefined;
}

const keyOf = (address: Address) => `${address.machine}\n${address.resident}`;

/** The page's two views: what needs the reader now, under the drawing; and
 *  what came back lately and is at work. */
type View = "now" | "lately";

const VIEW_KEY = "front-view";

function storedView(): View {
  try {
    return localStorage.getItem(VIEW_KEY) === "lately" ? "lately" : "now";
  } catch {
    return "now";
  }
}

function keepView(view: View): void {
  try {
    if (view === "lately") localStorage.setItem(VIEW_KEY, view);
    else localStorage.removeItem(VIEW_KEY);
  } catch {
    // Not kept: the page opens on now next time.
  }
}
const addressOf = (r: ResidentNow): Address => ({ machine: r.about.machine, resident: r.about.id });

export class HomePage {
  readonly element: HTMLElement;
  private world: WorldState | null = null;
  private now = new Date();
  private drawn = "";
  private readonly board: HTMLElement;
  /** The command whose stop is being asked once more, and what became of
   *  stops that did not go, by command. */
  private confirming: string | null = null;
  private stopping = new Set<string>();
  private readonly notStopped = new Map<string, string>();
  private view: View = storedView();
  /** Whether there is a network to draw. */
  private drawable = true;

  /** Shows the drawing of the World, or leaves the page to the board where
   *  there is no network to draw. */
  showDrawing(shown: boolean): void {
    this.drawable = shown;
    this.element.querySelector<HTMLElement>(".front-map")!.hidden = !shown || this.view !== "now";
  }

  constructor(private readonly options: HomeOptions) {
    this.element = document.createElement("div");
    this.element.className = "front-page";
    this.element.innerHTML = `${options.head}<div class="front-body"><div class="front-map"></div><div class="front-board arriving"></div></div>`;
    // The two views are chosen in the head, where the page has no title.
    const views = document.createElement("div");
    views.className = "front-views";
    views.setAttribute("role", "tablist");
    views.innerHTML = `<button class="tab" type="button" role="tab" data-view="now">${escape(options.words.nowView)}</button><button class="tab" type="button" role="tab" data-view="lately">${escape(options.words.latelyView)}</button>`;
    this.element.querySelector(".page-head-title")!.append(views);
    views.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLElement>("[data-view]");
      if (!button) return;
      this.view = button.dataset.view as View;
      keepView(this.view);
      this.choose();
      this.redraw();
    });
    this.element.querySelector<HTMLElement>(".front-map")!.append(options.drawing);
    this.board = this.element.querySelector<HTMLElement>(".front-board")!;
    // On a phone the page scrolls whole: once it has, its head stands on the
    // paper, so what scrolls passes under it.
    const body = this.element.querySelector<HTMLElement>(".front-body")!;
    body.addEventListener("scroll", () => this.element.classList.toggle("scrolled", body.scrollTop > 4), { passive: true });
    this.board.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest(".front-begin")) {
        options.newTalk();
        return;
      }
      if (target.closest("[data-front-updates]")) {
        options.updates?.();
        return;
      }
      const stop = target.closest<HTMLElement>("[data-job]");
      if (stop) void this.stopJob(stop);
    });
    // A mark on the line of the time away and its conversation's line light
    // together.
    const light = (event: Event, on: boolean) => {
      const key = (event.target as HTMLElement).closest<HTMLElement>("[data-came]")?.dataset.came;
      if (!key) return;
      for (const element of this.board.querySelectorAll<HTMLElement>("[data-came]")) {
        if (element.dataset.came === key) element.classList.toggle("lit", on);
      }
    };
    this.board.addEventListener("pointerover", (event) => light(event, true));
    this.board.addEventListener("pointerout", (event) => light(event, false));
    // What is on the board rises into place when the page opens, and only
    // then: later changes appear where they are.
    setTimeout(() => this.board.classList.remove("arriving"), 1600);
    this.choose();
  }

  private choose(): void {
    for (const button of this.element.querySelectorAll<HTMLElement>(".front-views [data-view]")) {
      const chosen = button.dataset.view === this.view;
      button.classList.toggle("chosen", chosen);
      button.setAttribute("aria-selected", String(chosen));
    }
    this.element.querySelector<HTMLElement>(".front-map")!.hidden = !this.drawable || this.view !== "now";
  }

  /** Shows the World as it now stands; when nothing shown has changed, the
   *  page is left as it is. */
  update(world: WorldState, now: Date): void {
    this.world = world;
    this.now = now;
    const residents = this.residents();
    const asking = residents.filter((r) => r.now?.waiting_for_user || r.now?.asked).sort((a, b) => waited(a) - waited(b));
    // What the conversations the user began told them that they have not
    // seen: the urgent first, then what can wait, the latest first.
    const telling = residents
      .filter((r) => r.about.origin.kind === "user" && this.unseen(r))
      .sort((a, b) => Number(b.now!.told!.status === "urgent") - Number(a.now!.told!.status === "urgent") || Date.parse(b.now!.told!.at) - Date.parse(a.now!.told!.at));
    const asks = new Set([...asking, ...telling].map((r) => keyOf(addressOf(r))));
    const busy = residents
      .filter((r) => r.now?.activity === "busy" && !asks.has(keyOf(addressOf(r))))
      .sort((a, b) => Date.parse(a.now!.since) - Date.parse(b.now!.since));
    const jobs = world.machines
      .flatMap((machine) => (machine.kind === "answered" ? machine.reply.jobs : []))
      .sort((a, b) => Date.parse(a.started_at) - Date.parse(b.started_at));
    const came = this.came(asks);

    if (this.view === "now") {
      // Nothing waits: the drawing alone, and a line that says so.
      const needs = asking.length || telling.length ? this.asks(asking, telling) : `<p class="front-quiet front-calm">${escape(this.options.words.nothingNeeds)}</p>`;
      this.draw(`<div class="front-column">${needs}</div>`, false);
      return;
    }
    const first = this.away(came);
    const second = this.working(busy, jobs);
    const html = first ? `<div class="front-column">${first}</div><div class="front-column">${second}</div>` : `<div class="front-column">${second}</div>`;
    this.draw(html, !!first);
  }

  /** The World could not be asked: said where the board would be. `old` is
   *  the World's version where it is older than the interface, which is then
   *  what is said instead of the refusal itself. */
  unreachable(why: string, now: Date, old: VersionMismatch | null = null): void {
    const { words } = this.options;
    this.now = now;
    const html = old
      ? `<div class="front-column"><p class="front-trouble">${escape(words.oldWorld(old.node_version, old.client_version))}</p><button class="front-begin-link" type="button" data-front-updates>${escape(words.renewThere)}</button></div>`
      : `<div class="front-column"><p class="front-trouble" title="${escape(why)}">${escape(words.unreachable(why))}</p><a class="front-begin-link" href="#/login">${escape(words.setUp)}</a></div>`;
    this.draw(html, false);
  }

  private draw(html: string, two: boolean): void {
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.board, () => {
      this.board.classList.toggle("two", two);
      this.board.innerHTML = html;
    });
  }

  private residents(): ResidentNow[] {
    return (this.world?.machines ?? []).flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : [])).filter((r) => !r.removed);
  }

  private knownAt(address: Address): ResidentNow | undefined {
    return this.world?.known.find((r) => r.about.id === address.resident && r.about.machine === address.machine);
  }

  /** What it is called: its conversation's name, or who it is. */
  private name(r: ResidentNow): string {
    return titleOf(r) ?? this.someone(r);
  }

  private nameOf(address: Address): string {
    const known = this.knownAt(address);
    return known ? this.name(known) : `${address.resident} ${address.machine}`;
  }

  /** What a pointer resting on a conversation's name is told beyond the name
   *  it sees: who it is and where, the project it is in, and whose it was
   *  called by. */
  private about(r: ResidentNow | undefined, name: string): string {
    if (!r) return name;
    const { words } = this.options;
    const who = this.someone(r);
    const project = r.town ? (this.world?.towns.find((town) => town.town === r.town)?.record.name ?? r.town) : "";
    const called = r.about.origin.kind === "colleague" ? words.colleagueOf(this.nameOf(r.about.origin.asked_by)) : "";
    return [name, who === name ? "" : who, project, called].filter(Boolean).join("\n");
  }

  /** The agent's mark: Claude's or Codex's, another's in general. */
  private agent(r: ResidentNow | undefined): string {
    const agent = r?.about.host.agent;
    const title = agent ? agentName(agent) : "";
    return `<span class="front-agent" title="${escape(title)}">${glyph(agent === "claude" || agent === "codex" ? agent : "agents", 16)}</span>`;
  }

  /** Who it is: the agent on its machine, or for one assisting the user,
   *  whose agent, never the machine. */
  private someone(r: ResidentNow): string {
    const { words } = this.options;
    const agent = agentName(r.about.host.agent);
    const owner = this.world?.assisting.get(r.about.machine);
    return owner !== undefined ? words.someoneAssisting(agent, owner) : words.someone(agent, r.about.machine);
  }

  /** The machine's mark, as the sidebar draws it, named on hover; where
   *  agents assisting the user run, a person's, named as whose agents. */
  private device(machine: string): string {
    const owner = this.world?.assisting.get(machine);
    if (owner !== undefined) return `<span class="front-device" title="${escape(this.options.words.assistAgents(owner))}">${glyph("colleague", 16)}</span>`;
    const node = this.world?.nodes.find((one) => one.name === machine);
    const form = node && "Ok" in node.facts ? (node.facts.Ok.form ?? "desktop") : "desktop";
    return `<span class="front-device" title="${escape(machine)}">${glyph(form, 16)}</span>`;
  }

  /** A moment, as short as it can be said from now. */
  private when(at: Date): string {
    const { words } = this.options;
    if (at.toDateString() === this.now.toDateString()) return words.clock(at);
    const before = new Date(this.now);
    before.setDate(before.getDate() - 1);
    return words.earlier(at, at.toDateString() === before.toDateString());
  }

  // What waits for the reader --------------------------------------------

  private asks(asking: ResidentNow[], telling: ResidentNow[]): string {
    const { words } = this.options;
    return `<section class="front-part front-asks"><h2 class="front-title"><span>${escape(words.waiting)}</span><span class="front-count">${asking.length + telling.length}</span></h2><ul class="front-cards">${[
      ...asking.map((r) => this.ask(r)),
      ...telling.map((r) => this.told(r)),
    ].join("")}</ul></section>`;
  }

  /** What a conversation told the user that this device has not shown
   *  them: something urgent, or something that can wait. */
  private unseen(r: ResidentNow): boolean {
    const told = r.now?.told;
    return !!told && told.status !== "reply" && this.options.seenTold(`${r.about.id}@${r.about.machine}`) !== told.id;
  }

  /** A card of what it told: who, when, and its words, an urgent one under
   *  a clay ※. */
  private told(r: ResidentNow): string {
    const { words } = this.options;
    const address = addressOf(r);
    const name = this.name(r);
    const told = r.now!.told!;
    const at = new Date(told.at);
    const urgent = told.status === "urgent";
    // The card says the first paragraph; held or pointed at, the whole.
    const shown = gist(told.text);
    const whole = shown === told.text.trim() ? "" : ` title="${escape(told.text)}"`;
    return `<li class="front-card" data-machine="${escape(address.machine)}" data-resident="${escape(address.resident)}">
      <a class="front-card-link" href="${this.options.link(address)}">
        <span class="front-card-head">${this.agent(r)}<span class="front-name" title="${escape(this.about(r, name))}">${escape(name)}</span>${this.device(r.about.machine)}<span class="front-for" title="${escape(this.when(at))}">${escape(words.ago(Math.max(0, this.now.getTime() - at.getTime())))}</span></span>
        <span class="front-ask front-told${urgent ? " urgent" : ""}">${urgent ? `<span class="front-ask-glyph" aria-hidden="true">※</span><span class="spoken">${escape(words.urgent)}</span>` : `<span class="front-ask-glyph" aria-hidden="true"><span class="front-told-drop"></span></span>`}<span class="front-ask-text"${whole}>${escape(shown)}</span></span>
      </a>
    </li>`;
  }

  private ask(r: ResidentNow): string {
    const { words } = this.options;
    const address = addressOf(r);
    const name = this.name(r);
    const asked = r.now?.asked;
    const since = r.now?.waiting_since ?? asked?.at ?? r.now?.since ?? r.about.born_at;
    const waitedFor = words.lasted(Math.max(0, this.now.getTime() - Date.parse(since)));
    const what = asked
      ? `<span class="front-ask${asked.what === "command" ? " code" : ""}"><span class="front-ask-glyph" aria-hidden="true">${glyph(ASK_GLYPHS[asked.what], 16)}</span><span class="spoken">${escape(words.asks[asked.what])}</span><span class="front-ask-text">${escape(asked.text)}</span></span>`
      : `<span class="front-ask quiet"><span class="front-ask-glyph" aria-hidden="true">${glyph("ask", 16)}</span><span class="front-ask-text">${escape(words.waitsUnsaid)}</span></span>`;
    return `<li class="front-card" data-machine="${escape(address.machine)}" data-resident="${escape(address.resident)}">
      <a class="front-card-link" href="${this.options.link(address)}">
        <span class="front-card-head">${this.agent(r)}<span class="front-name" title="${escape(this.about(r, name))}">${escape(name)}</span>${this.device(r.about.machine)}<span class="front-for" title="${escape(this.when(new Date(since)))}">${escape(waitedFor)}</span></span>
        ${what}
      </a>
    </li>`;
  }

  private redraw(): void {
    if (!this.world) return;
    this.drawn = "";
    this.update(this.world, this.now);
  }

  // What came back while the reader was away -----------------------------

  /** Each conversation's last turn that ended since the reader last looked,
   *  the latest first: none of one asked to help another, which answers to
   *  it, nor of one that waits for the reader, which says so above. */
  private came(asks: Set<string>): Came[] {
    const away = this.world?.away;
    const since = this.options.lastVisit?.getTime();
    if (!away || since === undefined) return [];
    const latest = new Map<string, Came>();
    for (const event of away.happenings) {
      if (event.kind !== "turn_ended") continue;
      const at = Date.parse(event.at);
      if (at < since) continue;
      const key = keyOf(event.who);
      const r = this.knownAt(event.who);
      if (asks.has(key) || r?.about.origin.kind === "colleague" || r?.removed) continue;
      const kept = latest.get(key);
      if (kept && kept.at.getTime() >= at) continue;
      latest.set(key, this.cameOf(event, key, r));
    }
    return [...latest.values()].sort((a, b) => b.at.getTime() - a.at.getTime());
  }

  private cameOf(event: Extract<Happening, { kind: "turn_ended" }>, key: string, r: ResidentNow | undefined): Came {
    const failed = event.how !== "completed";
    const words = this.options.words;
    const said = failed
      ? words.failed(event.reason ?? event.how)
      : (event.said ?? (r?.now?.last_compacted ? words.compacted : r?.now?.last_said) ?? "");
    return { key, address: event.who, at: new Date(event.at), failed, said, r };
  }

  private away(came: Came[]): string {
    const { words, lastVisit } = this.options;
    const away = this.world?.away;
    if (!lastVisit || !away) return "";
    // A machine away is not asked, and not said: machines come and go, and
    // there is nothing in that for the user to do. Only one that is there
    // and could not answer is.
    const unanswered = away.unasked.filter((u) => !u.offline);
    const missing = away.trouble
      ? `<p class="front-missing wrong" title="${escape(away.trouble)}">${escape(words.awayTrouble(away.trouble))}</p>`
      : unanswered.length
        ? `<p class="front-missing">${escape(words.unasked(unanswered.map((u) => words.unaskedWhy(u.machine, u.why ?? "")).join(words.listSeparator)))}</p>`
        : "";
    const span = words.lasted(Math.max(0, this.now.getTime() - lastVisit.getTime()));
    const list = came.length
      ? `<ol class="front-results">${came.map((one) => this.result(one)).join("")}</ol>`
      : `<p class="front-quiet">${escape(words.nothingAway)}</p>`;
    // The line of the time away is drawn where something came back along it.
    const line = came.length ? this.timeline(came, lastVisit) : "";
    return `<section class="front-part front-away"><h2 class="front-title"><span>${escape(words.away)}</span><span class="front-count">${escape(span)}</span></h2>${line}${list}${missing}</section>`;
  }

  /** The time away as a line drawn against a rule, from the last look to
   *  now, with a blot where each conversation last came back: red where it
   *  ended in an error. */
  private timeline(came: Came[], from: Date): string {
    const { words } = this.options;
    const start = from.getTime();
    const span = Math.max(1, this.now.getTime() - start);
    const random = seeded(`away:${start}`);
    const line = ruled([2, 6], [998, 6], 1.6, random);
    const marks = came
      .map((one) => {
        const left = Math.min(100, Math.max(0, ((one.at.getTime() - start) / span) * 100));
        const name = this.nameOf(one.address);
        const mark = blot([7, 7], 3.6, seeded(one.key));
        return `<a class="front-mark${one.failed ? " failed" : ""}" data-came="${escape(one.key)}" href="${this.options.link(one.address)}" style="left:${left.toFixed(2)}%" title="${escape(`${name} ${this.when(one.at)}`)}" aria-label="${escape(name)}"><svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true"><path d="${mark}" fill="currentColor"/></svg></a>`;
      })
      .join("");
    return `<div class="front-timeline"><svg class="front-rule" viewBox="0 0 1000 12" preserveAspectRatio="none" aria-hidden="true"><path d="${line}" fill="currentColor"/></svg>${marks}<span class="front-from">${escape(this.when(from))}</span><span class="front-to">${escape(words.present)}</span></div>`;
  }

  private result(one: Came): string {
    const name = this.nameOf(one.address);
    return `<li class="front-result${one.failed ? " failed" : ""}" data-came="${escape(one.key)}"><a href="${this.options.link(one.address)}">
      ${this.agent(one.r)}<span class="front-name" title="${escape(this.about(one.r, name))}">${escape(name)}</span><time class="front-at" datetime="${escape(one.at.toISOString())}">${escape(this.when(one.at))}</time>
      ${one.said ? `<span class="front-said">${escape(one.said)}</span>` : ""}
    </a></li>`;
  }

  // What is at work now ---------------------------------------------------

  private working(busy: ResidentNow[], jobs: RunningJob[]): string {
    const { words } = this.options;
    // One asked to help another hangs under it, where that one is at work.
    const keys = new Set(busy.map((r) => keyOf(addressOf(r))));
    const under = new Map<string, ResidentNow[]>();
    const top: ResidentNow[] = [];
    for (const r of busy) {
      const asker = r.about.origin.kind === "colleague" ? keyOf(r.about.origin.asked_by) : null;
      if (asker && keys.has(asker)) under.set(asker, [...(under.get(asker) ?? []), r]);
      else top.push(r);
    }
    const rows = top.length
      ? `<ul class="front-rows">${top.map((r) => this.row(r, under.get(keyOf(addressOf(r))) ?? [])).join("")}</ul>`
      : `<div class="front-quiet"><span>${escape(words.nobodyWorking)}</span><button class="front-begin" type="button">${glyph("plus", 14)}<span>${escape(words.begin)}</span></button></div>`;
    const running = jobs.length
      ? `<h3 class="front-subtitle"><span>${escape(words.jobs)}</span><span class="front-count">${jobs.length}</span></h3><ul class="front-jobs">${jobs.map((job) => this.job(job)).join("")}</ul>`
      : "";
    return `<section class="front-part front-now"><h2 class="front-title"><span>${escape(words.now)}</span><span class="front-count">${busy.length || ""}</span></h2>${rows}${running}</section>`;
  }

  private row(r: ResidentNow, called: ResidentNow[]): string {
    const { words } = this.options;
    const name = this.name(r);
    const trouble = r.now?.trouble;
    const lasted = words.lasted(Math.max(0, this.now.getTime() - Date.parse(r.now!.since)));
    const pulse = `<span class="front-pulse${trouble ? " troubled" : ""}"${trouble ? ` title="${escape(`${trouble.text}\n${words.ago(this.now.getTime() - Date.parse(trouble.at))}`)}"` : ""} aria-hidden="true"></span>`;
    const rest = called.length ? `<ul class="front-rows front-called">${called.map((one) => this.row(one, [])).join("")}</ul>` : "";
    return `<li><a class="front-row" href="${this.options.link(addressOf(r))}">
      ${pulse}${this.agent(r)}<span class="front-name" title="${escape(this.about(r, name))}">${escape(name)}</span>${this.device(r.about.machine)}<span class="front-for">${escape(lasted)}</span>
    </a>${rest}</li>`;
  }

  /** A command's stop: asked once more, then done; why it did not go is
   *  said in its place. */
  private async stopJob(button: HTMLElement): Promise<void> {
    const key = button.dataset.job!;
    const act = button.dataset.act;
    if (act === "ask") this.confirming = key;
    if (act === "cancel") this.confirming = null;
    this.notStopped.delete(key);
    if (act === "stop" && this.options.stopJob && !this.stopping.has(key)) {
      const [machine, job] = key.split("\n") as [string, string];
      this.stopping.add(key);
      this.redraw();
      try {
        await this.options.stopJob(job, machine);
        if (this.confirming === key) this.confirming = null;
      } catch (error) {
        this.notStopped.set(key, error instanceof Error ? error.message : String(error));
      } finally {
        this.stopping.delete(key);
      }
    }
    this.redraw();
  }

  private job(job: RunningJob): string {
    const { words } = this.options;
    const owner = ownerResident(job.owner);
    const by = owner ? this.nameOf(owner) : words.aPage("page" in job.owner ? job.owner.page.page : "");
    const whose = worksFor(this.world?.others, job.machine, job.job);
    const lasted = words.lasted(Math.max(0, this.now.getTime() - Date.parse(job.started_at)));
    const first = job.command.split("\n")[0] ?? "";
    const about = [job.command, words.startedBy(by), whose ? words.forAccounts(whose) : ""].filter(Boolean).join("\n");
    const key = escape(`${job.machine}\n${job.job}`);
    const stop = this.options.stopJob
      ? `<button class="icon-button front-job-stop" type="button" data-job="${key}" data-act="ask" aria-label="${escape(words.stopJob)}" title="${escape(words.stopJob)}">${glyph("stop", 14)}</button>`
      : "";
    const why = this.notStopped.get(`${job.machine}\n${job.job}`);
    const busy = this.stopping.has(`${job.machine}\n${job.job}`) ? " disabled" : "";
    const asking =
      this.confirming === `${job.machine}\n${job.job}`
        ? `<div class="jobs-confirm"><span${why ? ` title="${escape(why)}"` : ""}>${escape(why ? `${words.jobNotStopped}: ${why}` : words.stopJobAsk)}</span><button class="ask-button" type="button" data-job="${key}" data-act="cancel">${escape(words.cancel)}</button><button class="ask-button ask-primary" type="button" data-job="${key}" data-act="stop"${busy}>${escape(words.stopJob)}</button></div>`
        : "";
    return `<li><div class="front-job-line"><a class="front-job" href="${escape(fileLink(job.machine, job.output, true))}" title="${escape(about)}">
      <span class="front-job-glyph" aria-hidden="true">${glyph("command", 16)}</span><code class="front-command">${escape(first)}</code>${whose ? `<span class="front-whose">${glyph("person", 12)}<span>${escape(whose)}</span></span>` : ""}${this.device(job.machine)}<span class="front-for">${escape(lasted)}</span>
    </a>${stop}</div>${asking}</li>`;
  }
}

/** How long a conversation has waited, as when it began: the oldest first. */
/** What a conversation told, as a card says it: its first paragraph,
 *  without the marks of Markdown. */
function gist(text: string): string {
  const first = text.split(/\n\s*\n/).find((paragraph) => paragraph.trim()) ?? "";
  return first
    .split("\n")
    .map((line) => line.replace(/^\s*(?:[-*+]\s+|\d+[.)]\s+|#{1,6}\s+|>\s?)/, ""))
    .join(" ")
    .replace(/(\*\*|__|`)/g, "")
    .trim();
}

function waited(r: ResidentNow): number {
  return Date.parse(r.now?.waiting_since ?? r.now?.asked?.at ?? r.now?.since ?? r.about.born_at);
}

/** A small mark of whom something runs for, when that is other accounts
 *  than the reader's: a person and the account's name, said whole to a
 *  screen reader. Empty when it is the reader's own. */
export function accountTag(accounts: string | null, said: (accounts: string) => string): string {
  if (!accounts) return "";
  const label = said(accounts);
  return `<span class="tag tag-account" title="${escape(label)}">${glyph("person", 12)}<span aria-hidden="true">${escape(accounts)}</span><span class="spoken">${escape(label)}</span></span>`;
}
