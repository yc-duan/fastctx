// A list of changed files as it is drawn wherever one stands (`gui-files`):
// the activity page, the end of a conversation's turn, the conversation's
// side, a machine's folders. A row is a mark saying whether the file is
// new, changed, gone or a copy; its path, the folder quiet and cut from its
// beginning, the name always whole; who changed it, where the list does
// not already say; the lines added in green and removed in red; and five
// squares for how they divide. A change's lines are drawn a row to a line,
// one column of numbers, a removed line under its old number.

import hljs from "highlight.js/lib/common";
import { language, type DiffRow } from "../conversation/diff";
import { agentGlyph, glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { agentName } from "../world/names";
import { makers, squares, type ChangedFile, type FileChange, type How } from "./model";
import type { FilesWords } from "./words";
import "./files.css";

/** What a session (`resident@machine`) is called here and where it is
 *  opened, where this device knows it as a conversation. */
export type Named = (session: string) => { name: string; link: string } | null;

/** The lines of a change drawn before the rest are left to the file's own
 *  page. */
export const DIFF_ROWS = 400;

export const figure = (n: number) => n.toLocaleString("en-US");

/** The lines added and removed, as they are written beside a file. */
export function figures(added: number, removed: number): string {
  return `<span class="cf-figures">${added ? `<span class="cf-add">+${figure(added)}</span>` : ""}${removed ? `<span class="cf-del">−${figure(removed)}</span>` : ""}</span>`;
}

/** The mark before a path: a small square holding a plus for a new file, a
 *  dot for a changed one, a minus for one gone, an arrow for a copy. */
export function mark(how: How, said: string): string {
  const inside =
    how === "added" ? `<path d="M8 4.75v6.5M4.75 8h6.5"/>` : how === "removed" ? `<path d="M4.75 8h6.5"/>` : how === "copied" ? `<path d="M4.75 8h6M8.5 5.5 11 8l-2.5 2.5"/>` : `<circle cx="8" cy="8" r="1.6" class="cf-dot"/>`;
  return `<svg class="cf-mark cf-${how}" viewBox="0 0 16 16" width="16" height="16" role="img" aria-label="${escape(said)}"><title>${escape(said)}</title><rect x="1.5" y="1.5" width="13" height="13" rx="3.5"/>${inside}</svg>`;
}

export function squaresOf(added: number, removed: number): string {
  return `<span class="cf-squares" aria-hidden="true">${squares(added, removed)
    .map((kind) => `<i class="${kind}"></i>`)
    .join("")}</span>`;
}

/** A path, its folder cut from the beginning where there is no room, its
 *  name whole. `under` is a folder the list already stands in, left out. */
export function pathOf(file: { path: string; folder: string; name: string }, under = ""): string {
  const folder = under && file.folder.startsWith(under) ? file.folder.slice(under.length) : file.folder;
  return `<span class="cf-path" title="${escape(file.path)}"><span class="cf-folder"><bdi>${escape(folder)}</bdi></span><b>${escape(file.name)}</b></span>`;
}

/** What a session is called: the conversation's name where this device
 *  knows it, else whose session it is. */
export function sessionName(maker: { session: string | null; agent: string | null }, words: FilesWords, named: Named | undefined): { name: string; link: string | null } {
  const known = maker.session ? (named?.(maker.session) ?? null) : null;
  if (known) return known;
  const agent = maker.agent ? agentName(maker.agent) : "";
  return { name: agent ? words.own(agent) : (maker.session?.split("@")[0] ?? ""), link: null };
}

/** Who made a change: the agent's mark, then the conversation's name, a
 *  link to it where `linked`. */
export function whoOf(maker: { session: string | null; agent: string | null; from?: string | null }, words: FilesWords, named: Named | undefined, linked = true, more = 0): string {
  const { name, link } = sessionName(maker, words, named);
  if (!name && !maker.agent) return "";
  const said = link && linked ? `<a href="${escape(link)}">${escape(name)}</a>` : `<span>${escape(name)}</span>`;
  const from = maker.from ? `<small>${escape(words.from(maker.from))}</small>` : "";
  const others = more > 0 ? `<small>${escape(words.others(more))}</small>` : "";
  return `<span class="cf-who">${maker.agent ? agentGlyph(maker.agent, 16) : ""}${said}${from}${others}</span>`;
}

export interface RowOptions {
  /** Attributes of the row itself: what pressing it does. */
  attrs: string;
  /** Who changed it, said on the row. */
  named?: Named | undefined;
  who?: boolean;
  /** Its machine, said on the row. */
  where?: boolean;
  /** A folder the list stands in. */
  under?: string;
  /** What opens under the row, and whether it is open. */
  fold?: string;
  open?: boolean;
  /** What stands at the row's end, after its lines: a button of its own. */
  act?: string;
}

/** One file of a list. */
export function fileRow(file: ChangedFile, words: FilesWords, options: RowOptions): string {
  const all = makers(file);
  const who = options.who && all[0] ? whoOf(all[0], words, options.named, false, all.length - 1) : "";
  const where = options.where ? `<span class="cf-where">${glyph("machine", 14)}<span>${escape(file.machine)}</span></span>` : "";
  const folding = options.fold !== undefined;
  return `<li class="cf-file${options.open ? " open" : ""}">
    <div class="cf-row" role="button" tabindex="0"${folding ? ` aria-expanded="${!!options.open}"` : ""} ${options.attrs}>
      ${mark(file.how, words.how[file.how])}
      ${pathOf(file, options.under)}
      ${where}${who}
      ${file.uncounted ? `<span class="cf-figures cf-none" title="${escape(words.view.uncounted)}">—</span>` : figures(file.added, file.removed)}
      ${squaresOf(file.added, file.removed)}${options.act ?? ""}
    </div>${folding ? `<div class="cf-fold"><div class="cf-fold-inner">${options.fold}</div></div>` : ""}
  </li>`;
}

/** A change's own line: when, by whom, how, how many lines. */
export function changeLine(change: FileChange, words: FilesWords, named: Named | undefined, now: number): string {
  const said = [change.how === "changed" ? "" : words.how[change.how], change.seen ? words.view.seenTag : ""].filter(Boolean).join(" · ");
  return `<time>${escape(words.stamp(change.at, now))}</time>${whoOf(change, words, named)}<span class="cf-how"${change.seen ? ` title="${escape(words.seen)}"` : ""}>${escape(said)}</span>${
    change.uncounted ? "" : figures(change.added, change.removed)
  }`;
}

/** A change's lines: a row to a line under one column of numbers, a removed
 *  line under its old number, the others under their new; the lines left
 *  alone between two parts counted on a row of their own. */
export function diffLines(rows: readonly DiffRow[], path: string, words: FilesWords, limit = DIFF_ROWS): string {
  const lang = language(path);
  const shown = rows.slice(0, limit);
  const digits = Math.max(2, String(shown.reduce((most, row) => Math.max(most, row.old ?? 0, row.new ?? 0), 0)).length);
  const drawn = shown
    .map((row) => {
      if (row.kind === "gap") return `<div class="cf-dl cf-gap"><span class="cf-dn"></span><span class="cf-ds"></span><span class="cf-dt">${escape(words.view.gap(row.hidden ?? null))}</span></div>`;
      const number = row.kind === "del" ? row.old : row.new;
      const sign = row.kind === "add" ? "+" : row.kind === "del" ? "−" : "";
      const text = lang && row.text ? hljs.highlight(row.text, { language: lang, ignoreIllegals: true }).value : escape(row.text);
      // How far the line is indented, a tab four: what is carried on under
      // it hangs a little past that.
      const indent = [...(/^[ \t]*/.exec(row.text)?.[0] ?? "")].reduce((sum, mark) => sum + (mark === "\t" ? 4 : 1), 0);
      return `<div class="cf-dl cf-${row.kind}"><span class="cf-dn">${number ?? ""}</span><span class="cf-ds">${sign}</span><span class="cf-dt"${indent ? ` style="--in:${Math.min(indent, 40)}"` : ""}>${text || " "}</span></div>`;
    })
    .join("");
  const rest = rows.length > limit ? `<p class="cf-longer">${escape(words.view.longer(rows.length - limit))}</p>` : "";
  return `<div class="cf-diff" style="--line-digits:${digits}"><div class="cf-diff-lines">${drawn}</div></div>${rest}`;
}
