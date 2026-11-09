// The places the interface shows, and the address of each: what the hash
// in the window's address says, and what the sidebar marks as current.

import type { Address } from "../world/events";
import type { AgentSection, AgentTab } from "./profile-words";

/** The features a World can turn on (`world-gui.md`, 功能). */
export type Feature = "conversations" | "collaboration" | "sentinels" | "history";
const features: Feature[] = ["conversations", "collaboration", "sentinels", "history"];
const sections: AgentSection[] = ["claude", "codex", "prompt", "skills", "mcp"];

export type Place =
  | { kind: "home" }
  /** What agents did with run on the user's machines (`world-layers.md`). */
  | { kind: "activity" }
  /** Connect or the workbench (`world-layers.md`). */
  | { kind: "mode" }
  | { kind: "talk"; address: Address }
  /** Beginning a conversation, in a project or outside any. */
  | { kind: "new"; town: string | null }
  | { kind: "project"; town: string }
  /** One document of a project's history (`world-history.md`), opened
   *  from the project or, `from`, from a conversation in it. */
  | { kind: "history"; town: string; name: string; from?: Address }
  /** One document of the history of a conversation outside any project. */
  | { kind: "talk-history"; address: Address; name: string }
  /** A page of a project: a version of it, or the current one. */
  | { kind: "page"; town: string; page: string; version: number | null }
  /** The drawing of the machines, with one chosen when `focus` names it;
   *  or their update view, with that one ticked. */
  | { kind: "machines"; focus?: string; updates?: boolean; alerts?: boolean }
  /** Every project of the user's accounts, managed as a whole. */
  | { kind: "projects" }
  /** A machine's own page: how it is doing, and what is done to it. */
  | { kind: "machine"; name: string }
  /** The agents: a section of them, the one last shown when none is
   *  named; a host's section on one machine, or the last one shown, or
   *  the account's preferences for that host (`tab`). */
  | { kind: "agents"; machine: string | null; section?: AgentSection; tab?: AgentTab }
  /** The account's Claude subagents, come into from Claude Code's
   *  preferences. */
  | { kind: "subagents" }
  /** The account's prompt for one family, come into from its card of the
   *  models' settings. */
  | { kind: "prompt"; agent: "claude" | "codex" }
  /** What an agent's login on a machine was used for; opened on one
   *  turn's requests when a conversation asks for them. */
  | { kind: "usage"; machine: string; agent: "codex" | "claude"; dir: string; turn?: { conversation: string; turn: string } }
  /** The features, one card each, and a feature's own page. */
  | { kind: "features" }
  | { kind: "feature"; feature: Feature }
  /** Every conversation, or every one in a project: the ones the sidebar
   *  has no room for among them. */
  | { kind: "talks"; town: string | null }
  /** Every conversation the user archived, to look back at or restore. */
  | { kind: "archived" }
  | { kind: "settings" }
  /** What World keeps on this computer, and deleting it. */
  | { kind: "data" }
  /** Into a network: joining one, or making a new one; with neither, the
   *  page asks which. */
  | { kind: "login"; way?: "join" | "new" | "code" }
  /** The user's own things in the network shown: 「我」. */
  | { kind: "account" }
  /** A file on a machine, read whole, or its end when it keeps growing. */
  | { kind: "file"; machine: string; path: string; tail: boolean }
  /** What one of a conversation's watchers did, read on the machine it
   *  lives on: the life of it `generation` names. */
  | { kind: "sentinel"; home: string; id: string; generation: number }
  /** Every watcher of the user's accounts, ended or not: what each did
   *  over the last days, and what it took. */
  | { kind: "sentinels" };

const part = encodeURIComponent;

export function link(place: Place): string {
  switch (place.kind) {
    case "home":
      return "#/";
    case "activity":
      return "#/activity";
    case "mode":
      return "#/mode";
    case "talk":
      return `#/talk/${part(`${place.address.resident}@${place.address.machine}`)}`;
    case "new":
      return place.town === null ? "#/new" : `#/new/${part(place.town)}`;
    case "project":
      return `#/project/${part(place.town)}`;
    case "history":
      return `#/history/${part(place.town)}/${part(place.name)}${place.from ? `/from/${part(`${place.from.resident}@${place.from.machine}`)}` : ""}`;
    case "talk-history":
      return `#/talk-history/${part(`${place.address.resident}@${place.address.machine}`)}/${part(place.name)}`;
    case "page":
      return `#/page/${part(place.town)}/${part(place.page)}${place.version === null ? "" : `/${place.version}`}`;
    case "projects":
      return "#/projects";
    case "machines":
      if (place.updates) return place.focus ? `#/updates/${part(place.focus)}` : "#/updates";
      if (place.alerts) return "#/alerts";
      return place.focus ? `#/machines/${part(place.focus)}` : "#/machines";
    case "machine":
      return `#/machine/${part(place.name)}`;
    case "agents":
      if (place.tab === "prefs") return place.section === "claude" || place.section === "codex" ? `#/agents/prefs/${place.section}` : "#/agents/prefs";
      if (place.section) return `#/agents/${place.section}${place.machine === null ? "" : `/${part(place.machine)}`}`;
      return place.machine === null ? "#/agents" : `#/agents/${part(place.machine)}`;
    case "subagents":
      return "#/agents/subagents";
    case "prompt":
      return `#/agents/prompt/${place.agent}`;
    case "usage":
      return `#/agents/${part(place.machine)}/${place.agent}/${part(place.dir)}${place.turn ? `/calls/${part(place.turn.conversation)}/${part(place.turn.turn)}` : ""}`;
    case "features":
      return "#/features";
    case "feature":
      return `#/feature/${place.feature}`;
    case "talks":
      return place.town === null ? "#/talks" : `#/talks/${part(place.town)}`;
    case "archived":
      return "#/archived";
    case "settings":
      return "#/settings";
    case "data":
      return "#/data";
    case "login":
      return place.way ? `#/login/${place.way}` : "#/login";
    case "account":
      return "#/account";
    case "file":
      return `#/file/${part(place.machine)}/${place.tail ? "tail" : "whole"}/${part(place.path)}`;
    case "sentinel":
      return `#/sentinel/${part(place.home)}/${part(place.id)}/${place.generation}`;
    case "sentinels":
      return "#/sentinels";
  }
}

/** The place an address names; anything it does not name is the World. */
export function placeOf(hash: string): Place {
  const parts = hash.replace(/^#\/?/, "").split("/").map(decodeURIComponent);
  switch (parts[0]) {
    case "talk": {
      const name = parts.slice(1).join("/");
      const at = name.lastIndexOf("@");
      if (at > 0) return { kind: "talk", address: { resident: name.slice(0, at), machine: name.slice(at + 1) } };
      break;
    }
    case "new":
      return { kind: "new", town: parts[1] || null };
    case "project":
      if (parts[1]) return { kind: "project", town: parts[1] };
      break;
    case "history": {
      if (!parts[1] || !parts[2]) break;
      const at = parts[3] === "from" ? (parts[4]?.lastIndexOf("@") ?? -1) : -1;
      const from = at > 0 ? { resident: parts[4]!.slice(0, at), machine: parts[4]!.slice(at + 1) } : undefined;
      return { kind: "history", town: parts[1], name: parts[2], ...(from ? { from } : {}) };
    }
    case "talk-history": {
      const at = parts[1]?.lastIndexOf("@") ?? -1;
      if (parts[1] && parts[2] && at > 0) return { kind: "talk-history", address: { resident: parts[1].slice(0, at), machine: parts[1].slice(at + 1) }, name: parts[2] };
      break;
    }
    case "page":
      if (parts[1] && parts[2]) {
        const version = parts[3] ? Number(parts[3]) : null;
        return { kind: "page", town: parts[1], page: parts[2], version: version !== null && Number.isInteger(version) && version > 0 ? version : null };
      }
      break;
    case "projects":
      return { kind: "projects" };
    case "machines":
      return parts[1] ? { kind: "machines", focus: parts[1] } : { kind: "machines" };
    case "updates":
      return parts[1] ? { kind: "machines", focus: parts[1], updates: true } : { kind: "machines", updates: true };
    case "alerts":
      return { kind: "machines", alerts: true };
    case "machine":
      if (parts[1]) return { kind: "machine", name: parts[1] };
      break;
    case "agents":
      if (parts[1] && (parts[2] === "codex" || parts[2] === "claude") && parts[3]) {
        const turn = parts[4] === "calls" && parts[5] && parts[6] ? { turn: { conversation: parts[5], turn: parts[6] } } : {};
        return { kind: "usage", machine: parts[1], agent: parts[2], dir: parts[3], ...turn };
      }
      // A host's preferences; `#/agents/prefs` alone was the preferences'
      // own section, now the last host's.
      if (parts[1] === "prefs") return { kind: "agents", machine: null, tab: "prefs", ...(parts[2] === "claude" || parts[2] === "codex" ? { section: parts[2] } : {}) };
      if (parts[1] === "subagents") return { kind: "subagents" };
      // A family's prompt; the prompt every agent shared had a tab of its
      // own, now each family's card.
      if (parts[1] === "prompt") return parts[2] === "claude" || parts[2] === "codex" ? { kind: "prompt", agent: parts[2] } : { kind: "agents", machine: null, tab: "prefs" };
      if (sections.includes(parts[1] as AgentSection)) return { kind: "agents", section: parts[1] as AgentSection, machine: parts[2] || null };
      return { kind: "agents", machine: parts[1] || null };
    case "activity":
      return { kind: "activity" };
    case "mode":
      return { kind: "mode" };
    // The features page is gone: the workbench is a mode, and the other
    // features' settings stand where they are used.
    case "features":
      return { kind: "mode" };
    case "feature":
      if (parts[1] === "conversations") return { kind: "mode" };
      if (features.includes(parts[1] as Feature)) return { kind: "feature", feature: parts[1] as Feature };
      return { kind: "mode" };
    case "talks":
      return { kind: "talks", town: parts[1] || null };
    case "archived":
      return { kind: "archived" };
    case "settings":
      return { kind: "settings" };
    case "data":
      return { kind: "data" };
    // An address from before accounts, kept so old links still open.
    case "welcome":
    case "login":
      return parts[1] === "join" || parts[1] === "new" || parts[1] === "code" ? { kind: "login", way: parts[1] } : { kind: "login" };
    // A network's page and the administrator's are gone: what they held is
    // on 「我」 and on the drawing of the machines.
    case "network":
    case "account":
      return { kind: "account" };
    case "admin":
      return { kind: "machines" };
    case "file":
      if (parts[1] && parts[3]) return { kind: "file", machine: parts[1], path: parts.slice(3).join("/"), tail: parts[2] === "tail" };
      break;
    case "sentinel": {
      const generation = Number(parts[3]);
      if (parts[1] && parts[2] && Number.isSafeInteger(generation) && generation >= 0) return { kind: "sentinel", home: parts[1], id: parts[2], generation };
      break;
    }
    case "sentinels":
      return { kind: "sentinels" };
  }
  return { kind: "home" };
}

export const talkLink = (address: Address) => link({ kind: "talk", address });

/** Where a file on a machine is read: whole, or its end when it grows. */
export const fileLink = (machine: string, path: string, tail = false) => link({ kind: "file", machine, path, tail });
