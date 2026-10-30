// The agents' sections (`world-gui.md`, 智能体页): each agent on a page of
// its own, its logins and, beside them on a tab, the account's preferences
// for it, Claude Code's leading on to its subagents; then the rest of the
// account's own layer of configuration
// (`world-user-config.md`): its prompt, its skills and its MCP servers. While one of
// them is open they stand under 「智能体」 in the sidebar, a step in, as a
// project's conversations stand under it; the one last open, and the tab, are
// kept on this device, and 「智能体」 opens them again.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { link, type Place } from "./places";
import type { AgentSection, AgentTab, ProfileWords } from "./profile-words";

export const HOSTS = ["claude", "codex"] as const satisfies readonly AgentSection[];
export const OWN = ["prompt", "skills", "mcp"] as const satisfies readonly AgentSection[];

const SECTION = "agents.section";
const VIEW = "agents.view";
/** Where the logins page kept the agent it showed, before it had sections. */
const TAB = "agents.tab";

export function isHost(section: AgentSection): section is "claude" | "codex" {
  return section === "claude" || section === "codex";
}

/** The section last open; the agent last shown before there were any. */
export function keptSection(): AgentSection {
  try {
    const kept = localStorage.getItem(SECTION) as AgentSection | null;
    if (kept && ([...HOSTS, ...OWN] as AgentSection[]).includes(kept)) return kept;
    const tab = localStorage.getItem(TAB);
    if (tab === "codex" || tab === "claude") return tab;
  } catch {
    // A device that keeps nothing opens Claude Code.
  }
  return "claude";
}

/** The agent last shown, for a machine named with no section. */
export function keptHost(): "claude" | "codex" {
  const kept = keptSection();
  if (isHost(kept)) return kept;
  try {
    const tab = localStorage.getItem(TAB);
    if (tab === "codex" || tab === "claude") return tab;
  } catch {
    // As above.
  }
  return "claude";
}

export function keepSection(section: AgentSection): void {
  try {
    localStorage.setItem(SECTION, section);
    if (isHost(section)) localStorage.setItem(TAB, section);
  } catch {
    // Kept for this visit only.
  }
}

/** The tab of a host's page last shown. */
export function keptTab(): AgentTab {
  try {
    // The subagents were a tab once: who left them comes back to the
    // preferences they now open from.
    const kept = localStorage.getItem(VIEW);
    return kept === "prefs" || kept === "subagents" ? "prefs" : "logins";
  } catch {
    return "logins";
  }
}

export function keepTab(tab: AgentTab): void {
  try {
    localStorage.setItem(VIEW, tab);
  } catch {
    // Kept for this visit only.
  }
}

/** The section a place shows, when it is one of the agents'. A machine named
 *  with no section, as from a machine's page, is that machine's logins; the
 *  preferences with none, the last agent's. */
export function sectionOf(place: Place): AgentSection | null {
  if (place.kind === "usage") return place.agent;
  if (place.kind === "subagents") return "claude";
  if (place.kind !== "agents") return null;
  if (place.section) return place.section;
  if (place.tab === "prefs" || place.machine !== null) return keptHost();
  return keptSection();
}

/** The tab a place shows on a host's page: the preferences where it names
 *  them, the logins where it names a machine, else the one last shown. */
export function tabOf(place: Place): AgentTab {
  if (place.kind !== "agents") return "logins";
  if (place.tab) return place.tab;
  return place.machine === null && place.section === undefined ? keptTab() : "logins";
}

export function sectionMark(section: AgentSection, size = 16): string {
  switch (section) {
    case "claude":
      return glyph("claude", size);
    case "codex":
      return glyph("codex", size);
    case "prompt":
      return glyph("note", size);
    case "skills":
      return glyph("skill", size);
    case "mcp":
      return glyph("plug", size);
  }
}

/** The sections under 「智能体」 in the sidebar, `current` among them; the
 *  account's own only where the device is logged in to one. A host's row
 *  opens the tab last shown. On the workbench, model collaboration's
 *  settings stand last among them (`collab`), the kernels' own. */
export function sectionRows(current: AgentSection | null, own: boolean, words: ProfileWords, collab: { name: string; current: boolean } | null = null): string {
  const shown: AgentSection[] = own ? [...HOSTS, ...OWN] : [...HOSTS];
  const tab = own ? keptTab() : "logins";
  return `<ul class="asec-list">${shown
    .map((section) => {
      const on = section === current;
      const place: Place = isHost(section) && tab !== "logins" ? { kind: "agents", machine: null, section, tab } : { kind: "agents", machine: null, section };
      return `<li><a class="place asec-row${on ? " current" : ""}" href="${link(place)}"${on ? ' aria-current="page"' : ""}><span class="place-mark">${sectionMark(section)}</span><span class="place-name">${escape(words.sections[section])}</span></a></li>`;
    })
    .join("")}${
    collab
      ? `<li><a class="place asec-row${collab.current ? " current" : ""}" href="${link({ kind: "feature", feature: "collaboration" })}"${collab.current ? ' aria-current="page"' : ""}><span class="place-mark">${glyph("colleague", 16)}</span><span class="place-name">${escape(collab.name)}</span></a></li>`
      : ""
  }</ul>`;
}
