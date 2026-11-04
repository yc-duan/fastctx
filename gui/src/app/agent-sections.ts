// The agents' sections (`world-gui.md`, 智能体页): each agent on a page of
// its own, its logins and, beside them on a tab, the account's preferences
// for it, leading on to its prompt and, for Claude Code, its subagents; then
// the rest of the account's own layer of configuration
// (`world-user-config.md`): its skills and its MCP servers, which are the
// workbench conversations' settings (`./talk-settings`). The one last open
// is kept on this device.

import type { Place } from "./places";
import type { AgentSection, AgentTab } from "./profile-words";

export const HOSTS = ["claude", "codex"] as const satisfies readonly AgentSection[];
export const OWN = ["skills", "mcp"] as const satisfies readonly AgentSection[];

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
  if (place.kind === "prompt") return place.agent;
  if (place.kind !== "agents") return null;
  if (place.section === "prompt") return keptHost();
  if (place.section) return place.section;
  if (place.tab === "prefs" || place.machine !== null) return keptHost();
  return keptSection();
}
