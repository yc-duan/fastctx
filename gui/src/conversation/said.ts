// What a command answered, read for a person. The World's own controls are
// said in the interface's words; a host's own output is shown as it came,
// a short line beside the composer and anything longer in a panel above
// it; Codex's list of its tool servers is laid out as a list.

import type { Control, Offers } from "../world/control";
import type { JsonValue } from "../world/wire";
import { escape } from "./prose";

export interface SaidWords {
  model(model: string, effort: string | null): string;
  rewind: string;
  review: string;
  skill(name: string): string;
  close: string;
  skills: string;
  noSkills: string;
  mcp: string;
  noServers: string;
  /** A tool server's state, in the host's own word when it is not one of
   *  those the interface says in its own. */
  server(state: string): string;
  tools(count: number): string;
  toolsFailed(why: string): string;
}

export type Shown = { kind: "note"; text: string } | { kind: "panel"; title: string; html: string };

/** How a control that was done is said: in the interface's words for the
 *  World's own, as the host answered for the rest. */
export function shownOf(name: string, control: Control, said: string | null, words: SaidWords): Shown | null {
  switch (control.do) {
    case "model":
      return { kind: "note", text: words.model(control.model ?? "", control.effort) };
    case "compact":
      return { kind: "note", text: "" };
    case "rewind":
      return { kind: "note", text: words.rewind };
    case "rename":
      // The new name in the head says it.
      return { kind: "note", text: "" };
    case "fork":
    case "answer":
    case "agents":
    case "agent_messages":
    case "cancel_control":
      return null;
    case "command":
      if (control.path) return { kind: "note", text: words.skill(name) };
      if (name === "review") return { kind: "note", text: words.review };
      if (name === "mcp" && said !== null) {
        const servers = serversOf(said);
        if (servers) return { kind: "panel", title: words.mcp, html: serversHtml(servers, words) };
      }
      return said === null ? null : asSaid(`/${name}`, said);
  }
}

/** A host's own words: a short line beside the composer, anything longer
 *  in a panel, JSON laid out to be read. */
function asSaid(title: string, said: string): Shown {
  const text = said.trim();
  if (text.length <= 80 && !text.includes("\n")) return { kind: "note", text };
  return { kind: "panel", title, html: saidHtml(text) };
}

/** A host's output as it is read: JSON laid out, the rest as written. */
export function saidHtml(said: string): string {
  let body = said.trim();
  try {
    body = JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    // Not JSON: shown as the host wrote it.
  }
  return `<pre class="said-text">${escape(body)}</pre>`;
}

/** The skills a conversation offers, from what it offered. */
export function skillsHtml(offers: Offers | null, words: SaidWords): string {
  const skills = (offers?.commands ?? []).filter((command) => command.path);
  if (!skills.length) return `<div class="menu-empty">${escape(words.noSkills)}</div>`;
  return `<ul class="said-list">${skills
    .map((skill) => `<li><span class="said-name">/${escape(skill.name)}</span><span class="said-what">${escape(skill.description)}</span></li>`)
    .join("")}</ul>`;
}

interface Server {
  name: string;
  state: string;
  tools: number;
  error: string | null;
}

/** Codex's `mcpServerStatus/list` entries, when they have its shape. */
function serversOf(said: string): Server[] | null {
  let value: JsonValue;
  try {
    value = JSON.parse(said) as JsonValue;
  } catch {
    return null;
  }
  if (!Array.isArray(value)) return null;
  const servers: Server[] = [];
  for (const entry of value) {
    if (entry === null || typeof entry !== "object" || Array.isArray(entry) || typeof entry.name !== "string") return null;
    const tools = entry.tools !== null && typeof entry.tools === "object" && !Array.isArray(entry.tools) ? Object.keys(entry.tools).length : 0;
    servers.push({
      name: entry.name,
      state: typeof entry.runtimeStatus === "string" ? entry.runtimeStatus : "",
      tools,
      error: typeof entry.toolsError === "string" ? entry.toolsError : null,
    });
  }
  return servers;
}

function serversHtml(servers: Server[], words: SaidWords): string {
  if (!servers.length) return `<div class="menu-empty">${escape(words.noServers)}</div>`;
  return `<ul class="said-list">${servers
    .map(
      (server) =>
        `<li><span class="said-name">${escape(server.name)}</span><span class="said-what">${escape([server.state ? words.server(server.state) : "", words.tools(server.tools)].filter(Boolean).join(" "))}${server.error ? ` <span title="${escape(server.error)}">${escape(words.toolsFailed(server.error))}</span>` : ""}</span></li>`,
    )
    .join("")}</ul>`;
}
