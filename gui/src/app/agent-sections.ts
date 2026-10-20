// The agents' sections (`world-gui.md`, 智能体页): each agent's logins on a
// page of their own, then the account's own layer of configuration
// (`world-user-config.md`): its prompt, its MCP servers, its preferences.
// While one of them is open they stand under 「智能体」 in the sidebar, a
// step in, as a project's conversations stand under it; the one last open is
// kept on this device, and 「智能体」 opens it again.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { link, type Place } from "./places";
import type { AgentSection, ProfileWords } from "./profile-words";

export const HOSTS = ["claude", "codex"] as const satisfies readonly AgentSection[];
export const OWN = ["prompt", "mcp", "prefs"] as const satisfies readonly AgentSection[];

const SECTION = "agents.section";
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
function keptHost(): "claude" | "codex" {
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

/** The section a place shows, when it is one of the agents'. A machine named
 *  with no section, as from a machine's page, is that machine's logins. */
export function sectionOf(place: Place): AgentSection | null {
  if (place.kind === "usage") return place.agent;
  if (place.kind !== "agents") return null;
  if (place.section) return place.section;
  return place.machine === null ? keptSection() : keptHost();
}

export function sectionMark(section: AgentSection, size = 16): string {
  switch (section) {
    case "claude":
      return glyph("claude", size);
    case "codex":
      return glyph("codex", size);
    case "prompt":
      return glyph("note", size);
    case "mcp":
      return glyph("plug", size);
    case "prefs":
      return glyph("tune", size);
  }
}

/** The sections under 「智能体」 in the sidebar, `current` among them; the
 *  account's own only where the device is logged in to one. */
export function sectionRows(current: AgentSection | null, own: boolean, words: ProfileWords): string {
  const shown: AgentSection[] = own ? [...HOSTS, ...OWN] : [...HOSTS];
  return `<ul class="asec-list">${shown
    .map((section) => {
      const on = section === current;
      return `<li><a class="place asec-row${on ? " current" : ""}" href="${link({ kind: "agents", machine: null, section })}"${on ? ' aria-current="page"' : ""}><span class="place-mark">${sectionMark(section)}</span><span class="place-name">${escape(words.sections[section])}</span></a></li>`;
    })
    .join("")}</ul>`;
}
