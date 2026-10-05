// The sidebar: the places of the World, always at hand, so who is here can
// be seen from any page. The World itself; each project, open to the agents
// in it now with what each is doing; the agents in no project; the
// machines. A conversation is begun from here, and the settings are at its
// foot.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import type { Address } from "../world/events";
import type { ResidentNow } from "../world/world";
import type { Place } from "./places";
import { link } from "./places";
import { effect } from "./signals";
import { residentsOf, type Store } from "./store";
import type { ShellWords } from "./words";

export interface SidebarOptions {
  store: Store;
  words: ShellWords;
  /** The place shown now. */
  place(): Place;
  newTalk(): void;
  newProject(): void;
  fold(): void;
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

/** Projects folded shut by the reader, kept on this device. */
const folded = new Set<string>(
  (() => {
    try {
      return JSON.parse(localStorage.getItem("folded") ?? "[]") as string[];
    } catch {
      return [];
    }
  })(),
);

function keepFolded(): void {
  try {
    localStorage.setItem("folded", JSON.stringify([...folded]));
  } catch {
    // A device that keeps nothing opens every project again next time.
  }
}

export class Sidebar {
  readonly element: HTMLElement;
  private readonly list: HTMLElement;
  private drawn = "";

  constructor(private readonly options: SidebarOptions) {
    const { words } = options;
    this.element = document.createElement("nav");
    this.element.className = "sidebar";
    this.element.setAttribute("aria-label", words.app);
    this.element.innerHTML = `
      <div class="sidebar-head">
        <a class="wordmark" href="#/">${mark()}<span>${escape(words.app)}</span></a>
        <button class="icon-button sidebar-fold" type="button" aria-label="${escape(words.foldSidebar)}" title="${escape(words.foldSidebar)} · Ctrl+B">${glyph("sidebar", 18)}</button>
      </div>
      <button class="new-talk" type="button">${glyph("plus", 18)}<span>${escape(words.newTalk)}</span><kbd>Ctrl N</kbd></button>
      <div class="sidebar-places"></div>
      <div class="sidebar-foot">
        <a class="place settings" href="${link({ kind: "settings" })}">${glyph("settings", 18)}<span class="place-name">${escape(words.settings)}</span></a>
      </div>`;
    this.list = this.element.querySelector<HTMLElement>(".sidebar-places")!;
    this.element.querySelector(".new-talk")!.addEventListener("click", () => options.newTalk());
    this.element.querySelector(".sidebar-fold")!.addEventListener("click", () => options.fold());
    this.list.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-new-project]")) {
        options.newProject();
        return;
      }
      const toggle = (event.target as HTMLElement).closest<HTMLElement>("[data-fold]");
      if (!toggle) return;
      event.preventDefault();
      const town = toggle.dataset.fold!;
      if (folded.has(town)) folded.delete(town);
      else folded.add(town);
      keepFolded();
      this.drawn = "";
      this.draw();
    });
    effect(() => {
      options.store.world.get();
      options.store.trouble.get();
      options.store.clock.get();
      this.draw();
    });
  }

  /** Draws again, as when the place shown changed. */
  draw(): void {
    const { store, words } = this.options;
    const world = store.world.peek();
    const place = this.options.place();
    const now = store.clock.peek();
    const residents = residentsOf(world).filter((r) => r.here || r.now?.waiting_for_user);
    const waiting = residents.filter((r) => r.now?.waiting_for_user).length;

    const home = `<a class="place home${place.kind === "home" ? " current" : ""}" href="${link({ kind: "home" })}">
        <span class="place-mark">${knotMark()}</span><span class="place-name">${escape(words.world)}</span>
        ${waiting ? `<span class="count waiting"><span class="spoken">${escape(words.waiting)}</span>${waiting}</span>` : ""}
      </a>
      ${store.trouble.peek() ? `<p class="sidebar-trouble">${escape(words.unreachable)}</p>` : ""}`;

    const towns = world?.towns ?? [];
    const named = new Set(towns.map((town) => town.town));
    // A resident may live in a town this machine keeps no ledger of; it is
    // listed under its town's short name all the same.
    const unknown = [...new Set(residents.map((r) => r.town).filter((t): t is string => !!t && !named.has(t)))];
    const projectRows = [
      ...towns.map((town) => ({ town: town.town, name: town.record.name })),
      ...unknown.map((town) => ({ town, name: town })),
    ]
      .map(({ town, name }) => {
        const inside = order(residents.filter((r) => r.town === town));
        const shut = folded.has(town);
        const asking = inside.some((r) => r.now?.waiting_for_user);
        const current = (place.kind === "project" || place.kind === "journal" || place.kind === "page") && place.town === town;
        return `<li class="project-place${shut ? " shut" : ""}">
          <div class="place-row">
            <a class="place${current ? " current" : ""}" href="${link({ kind: "project", town })}"><span class="place-name">${escape(name)}</span>
              ${shut && asking ? `<span class="mark mark-ask" aria-hidden="true"></span>` : ""}
              ${shut && inside.length ? `<span class="count">${inside.length}</span>` : ""}</a>
            ${inside.length ? `<button class="place-fold" type="button" data-fold="${escape(town)}" aria-expanded="${!shut}" aria-label="${escape(words.agentsIn(name))}">${glyph("chevron", 16)}</button>` : ""}
          </div>
          ${shut || !inside.length ? "" : `<ul class="agents">${inside.map((r) => this.agent(r, place, now)).join("")}</ul>`}
        </li>`;
      })
      .join("");
    const projects = `<section class="sidebar-group"><h2 class="side-group-title">${escape(words.projects)}<button class="side-add" type="button" data-new-project aria-label="${escape(words.newProject)}" title="${escape(words.newProject)}">${glyph("plus", 14)}</button></h2>
      ${projectRows ? `<ul class="projects-places">${projectRows}</ul>` : `<p class="sidebar-quiet">${escape(words.noProjects)}</p>`}
    </section>`;

    const outside = order(residents.filter((r) => !r.town));
    const loose = outside.length
      ? `<section class="sidebar-group"><h2 class="side-group-title">${escape(words.outside)}</h2><ul class="agents loose">${outside.map((r) => this.agent(r, place, now)).join("")}</ul></section>`
      : "";

    const nodes = world?.nodes.machines ?? [];
    const machines = nodes.length
      ? `<section class="sidebar-group"><h2 class="side-group-title"><a href="${link({ kind: "machines" })}">${escape(words.machines)}</a></h2><ul class="machines-places">${nodes
          .filter((node) => !node.revoked)
          .map((node) => {
            const current = place.kind === "machine" && place.name === node.name;
            const state = node.online === true ? "online" : node.online === false ? "offline" : "unknown";
            const here = node.name === world?.here ? `<span class="place-note">${escape(words.thisOne)}</span>` : node.device ? `<span class="place-note">${escape(words.phone)}</span>` : "";
            return `<li><a class="place machine-place ${state}${current ? " current" : ""}" href="${link({ kind: "machine", name: node.name })}"><span class="dot" aria-hidden="true"></span><span class="place-name">${escape(node.name)}</span>${here}${node.online === false ? `<span class="place-note">${escape(words.offline)}</span>` : ""}</a></li>`;
          })
          .join("")}</ul></section>`
      : "";

    // On its own, this computer is a World of one; the way on is kept at
    // hand until it joins one.
    const alone =
      world && !world.nodes.world
        ? `<section class="sidebar-group"><a class="place${place.kind === "welcome" ? " current" : ""}" href="${link({ kind: "welcome" })}">${glyph("plus", 16)}<span class="place-name">${escape(words.joinOrFound)}</span></a></section>`
        : "";
    const html = home + projects + loose + machines + alone;
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.list, () => {
      this.list.innerHTML = html;
    });
  }

  private agent(r: ResidentNow, place: Place, now: Date): string {
    const { words } = this.options;
    const address: Address = { resident: r.about.id, machine: r.about.machine };
    const current = place.kind === "talk" && place.address.resident === address.resident && place.address.machine === address.machine;
    const name = words.someone(agentName(r.about.host.agent), r.about.machine);
    const waiting = r.now?.waiting_for_user;
    const busy = r.now?.activity === "busy";
    const title = r.about.origin.kind === "colleague" ? r.about.origin.label : name;
    const state = waiting
      ? words.waiting
      : busy
        ? words.busy(words.span(now.getTime() - Date.parse(r.now!.since)))
        : r.now?.activity === "asleep"
          ? words.asleep
          : words.idle;
    const meta = r.about.origin.kind === "colleague" ? `${name} · ${state}` : state;
    const kind = waiting ? "mark-ask" : busy ? "mark-busy" : "mark-idle";
    return `<li><a class="place agent-place ${kind}${current ? " current" : ""}" href="${link({ kind: "talk", address })}">
      <span class="mark ${kind}" aria-hidden="true"></span>
      <span class="agent-text"><span class="place-name">${escape(title)}</span><span class="place-meta">${escape(meta)}</span></span>
    </a></li>`;
  }
}

/** Who waits on the reader first, then who works, then the rest; within
 *  each, the most recently stirred first. */
function order(residents: ResidentNow[]): ResidentNow[] {
  const rank = (r: ResidentNow) => (r.now?.waiting_for_user ? 0 : r.now?.activity === "busy" ? 1 : 2);
  const stirred = (r: ResidentNow) => Date.parse(r.now?.since ?? r.about.born_at);
  return [...residents].sort((a, b) => rank(a) - rank(b) || stirred(b) - stirred(a));
}

/** The World's mark: machines as rings round the knot where they meet. */
function mark(): string {
  return `<svg class="wordmark-mark" width="22" height="22" viewBox="0 0 22 22" aria-hidden="true"><g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="11" cy="11" r="2"/><circle cx="4.5" cy="6" r="2.6"/><circle cx="17.5" cy="6.5" r="2.6"/><circle cx="11" cy="18" r="2.6"/><path d="M6.6 7.4L9.3 9.7M15.4 7.8l-2.7 1.9M11 13.1v2.3"/></g></svg>`;
}

function knotMark(): string {
  return `<svg width="18" height="18" viewBox="0 0 20 20" aria-hidden="true"><g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="10" cy="10" r="1.8"/><circle cx="4.6" cy="6" r="2.1"/><circle cx="15.6" cy="6.4" r="2.1"/><circle cx="10" cy="16.2" r="2.1"/></g></svg>`;
}
