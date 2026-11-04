// The settings of the workbench's conversations (`world-gui.md`, 接入与工作台):
// what the account gives every agent working for it (its skills, its MCP
// servers), each agent's own (the model it begins a conversation on, and
// its prompt, a step in from its card), and model collaboration. They reach only the conversations Fastctx holds,
// never an agent that is only connected, so they stand in the sidebar's
// workbench group as one place, its parts on tabs; connecting agents is the
// agents page's alone. The tab last open is kept on this device.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { HostPrefs } from "./host-prefs";
import { link, type Place } from "./places";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import type { TalkSettingsWords } from "./talk-settings-words";

export type SettingsTab = "skills" | "mcp" | "models" | "collab";
export type Host = "claude" | "codex";

const TABS: SettingsTab[] = ["skills", "mcp", "models", "collab"];
const HOSTS: Host[] = ["claude", "codex"];
const KEPT_TAB = "talk.settings";

/** The part of the settings a place shows; none where it is no part of them. */
export function settingsTabOf(place: Place): SettingsTab | null {
  if (place.kind === "agents" && (place.section === "skills" || place.section === "mcp")) return place.section;
  if (place.kind === "agents" && (place.tab === "prefs" || place.section === "prompt")) return "models";
  if (place.kind === "subagents" || place.kind === "prompt") return "models";
  if (place.kind === "feature" && (place.feature === "collaboration" || place.feature === "conversations")) return "collab";
  return null;
}

/** Where a part of the settings is. */
export function settingsPlace(tab: SettingsTab): Place {
  switch (tab) {
    case "skills":
    case "mcp":
      return { kind: "agents", machine: null, section: tab };
    case "models":
      return { kind: "agents", machine: null, section: "claude", tab: "prefs" };
    case "collab":
      return { kind: "feature", feature: "collaboration" };
  }
}

/** The settings as last left: the sidebar's way in. */
export function keptSettings(): Place {
  try {
    const kept = localStorage.getItem(KEPT_TAB) as SettingsTab | null;
    if (kept && TABS.includes(kept)) return settingsPlace(kept);
  } catch {
    // A device that keeps nothing opens the models.
  }
  return settingsPlace("models");
}

/** The head of one part of the settings: their name and the tabs, then
 *  whatever the part's own page sets beside its name in place of it. */
export function settingsHead(
  head: (title: string, actions?: string) => string,
  words: TalkSettingsWords,
  current: SettingsTab,
): (title: string, actions?: string) => string {
  try {
    localStorage.setItem(KEPT_TAB, current);
  } catch {
    // Kept for this visit only.
  }
  const tabs = `<nav class="segmented cset-tabs" data-tab-group="talk-settings" aria-label="${escape(words.title)}">${TABS.map(
    (tab) =>
      `<a class="segment${tab === current ? " chosen" : ""}" href="${link(settingsPlace(tab))}"${tab === current ? ' aria-current="page"' : ""}>${escape(words.tabs[tab])}</a>`,
  ).join("")}</nav>`;
  return (title, actions) => head(`<h1 class="page-title">${escape(words.title)}</h1>${tabs}${title.replace(/<h1 class="page-title[^"]*">[\s\S]*?<\/h1>/, "")}`, actions);
}

export interface ModelPrefsOptions {
  store: Store;
  words: TalkSettingsWords;
  head(title: string, actions?: string): string;
  make(agent: Host): HostPrefs;
}

/** The models tab: each agent's preferences on a card of its own, side by
 *  side, the agent's mark and name at its head with the account they are
 *  of and what became of the last choice. */
export class ModelPrefsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly prefs: HostPrefs[];
  private readonly stop: () => void;

  constructor(options: ModelPrefsOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "agents-page cset-models";
    this.element.innerHTML = `${settingsHead(options.head, words, "models")("")}
      <div class="cset-cards scroller">${HOSTS.map(
        (agent) => `<section class="cset-card" data-agent="${agent}">
          <header class="cset-card-head">
            <span class="cset-card-mark" aria-hidden="true">${glyph(agent, 22)}</span>
            <h2 class="cset-card-name">${escape(words.agents[agent])}</h2>
            <span class="cset-card-account"></span>
            <span class="cset-card-status"></span>
          </header>
        </section>`,
      ).join("")}</div>`;
    this.prefs = HOSTS.map((agent) => {
      const prefs = options.make(agent);
      const card = this.element.querySelector<HTMLElement>(`.cset-card[data-agent="${agent}"]`)!;
      card.querySelector(".cset-card-account")!.replaceWith(prefs.account);
      card.querySelector(".cset-card-status")!.replaceWith(prefs.status);
      prefs.element.classList.remove("scroller");
      prefs.element.hidden = false;
      card.append(prefs.element);
      return prefs;
    });
    this.stop = effect(() => {
      const world = options.store.world.get();
      for (const prefs of this.prefs) prefs.draw(world);
    });
    for (const prefs of this.prefs) prefs.shown();
  }

  leave(): void {
    this.stop();
    for (const prefs of this.prefs) prefs.leave();
  }
}
