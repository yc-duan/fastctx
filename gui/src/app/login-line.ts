// One login of an agent as a line on paper (`world-gui.md`, 智能体): whether
// Fastctx is connected to it, drawn by its plug, ink or faint; who it is;
// what to know of it, quietly; and at its end the one thing to do with it.
// The agents' overview and an agent's own page draw their logins with it.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import "./login-line.css";

/** Connected; found and not yet connected; connected and asking to be
 *  renewed; being connected or taken out. */
export type Standing = "in" | "found" | "renew" | "busy";

/** What to know of a login, said in a word or two, with more on hover. */
export interface LineTag {
  text: string;
  said?: string;
  /** Something the user has to act on. */
  warn?: boolean;
}

export interface Line {
  standing: Standing;
  who: string;
  /** Said when the pointer rests on who it is: its directory. */
  whoSaid: string;
  tags: LineTag[];
  /** The deed at its end, drawn by the page. */
  act: string;
  /** The rest of the line's class attribute, as `freshness` writes it, for
   *  a line just added. */
  fresh?: string;
  /** Drawn below the line: a note on it. */
  below?: string;
}

export function loginLine(line: Line): string {
  const tags = line.tags
    .map((tag) => `<span class="ll-tag${tag.warn ? " ll-warn" : ""}"${tag.said ? ` title="${escape(tag.said)}"` : ""}>${escape(tag.text)}</span>`)
    .join("");
  return `<li class="ll-line${line.fresh ?? `"`} data-standing="${line.standing}">
    <span class="ll-row">
      <span class="ll-plug" aria-hidden="true">${glyph("plug", 16)}</span>
      <span class="ll-who" title="${escape(line.whoSaid)}">${escape(line.who)}</span>${tags ? `<span class="ll-tags">${tags}</span>` : ""}
      <span class="ll-act">${line.act}</span>
    </span>${line.below ?? ""}
  </li>`;
}
