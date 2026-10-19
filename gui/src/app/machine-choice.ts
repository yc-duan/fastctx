// Every machine one may choose among, set out as the machine list sets
// them (`machine-list.ts`): the user's own first, then each other person's,
// under each a quiet name and a pencil line; in each, the machines in no
// group, then each group its owner made, a row that opens and closes onto its
// machines, as a project does in the sidebar. A group of more than a dozen
// waits closed unless a machine in it is chosen, and opens where what is
// typed is looked for. In each group the machines there come first; one away
// is grey, and that is all that says so. Under its group, a machine named
// after it is written without the group's name. Shared by the machines and
// folders a conversation is pointed at (`focus-panel.ts`) and the machines a
// project spans (`project-form.ts`).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { FocusPlace } from "./focus-panel";

export interface MachineChoiceWords {
  here: string;
  offline: string;
  mine: string;
  theirs(owner: string): string;
}

/** A group opens by itself up to this many machines. */
const OPEN_GROUP = 12;

const FOLDS = "focusFolds";

/** The groups opened or closed, as they were left on this device. */
export function keptFolds(): Map<string, boolean> {
  try {
    const kept = JSON.parse(localStorage.getItem(FOLDS) ?? "[]") as unknown;
    return new Map(Array.isArray(kept) ? kept.filter((one): one is [string, boolean] => Array.isArray(one) && typeof one[0] === "string" && typeof one[1] === "boolean") : []);
  } catch {
    return new Map();
  }
}

function keepFolds(folds: Map<string, boolean>): void {
  try {
    localStorage.setItem(FOLDS, JSON.stringify([...folds]));
  } catch {
    // Not kept: the groups open as they would.
  }
}

/** The machines of one person, those in no group and each group's. */
interface Holding {
  owner: string | null;
  loose: FocusPlace[];
  groups: { label: string; places: FocusPlace[] }[];
}

const byName = (a: FocusPlace, b: FocusPlace) =>
  Number(b.here) - Number(a.here) || Number(b.online !== false) - Number(a.online !== false) || a.written.localeCompare(b.written, undefined, { numeric: true });

/** The machines as rows to choose by `data-act="toggle"`, groups to open by
 *  `data-act="fold"`; empty where nothing holds `query`. */
export function machineChoice(places: FocusPlace[], chosen: ReadonlySet<string>, query: string, folds: ReadonlyMap<string, boolean>, words: MachineChoiceWords): string {
  const holdings: Holding[] = [];
  for (const place of places) {
    const owner = place.owner ?? null;
    let holding = holdings.find((one) => one.owner === owner);
    if (!holding) holdings.push((holding = { owner, loose: [], groups: [] }));
    if (!place.group) holding.loose.push(place);
    else {
      let group = holding.groups.find((one) => one.label === place.group);
      if (!group) holding.groups.push((group = { label: place.group, places: [] }));
      group.places.push(place);
    }
  }
  holdings.sort((a, b) => (a.owner === null ? -1 : b.owner === null ? 1 : a.owner.localeCompare(b.owner)));
  const matches = (place: FocusPlace, group: string | null) => !query || place.written.toLowerCase().includes(query) || (!!group && group.toLowerCase().includes(query));
  const row = (place: FocusPlace, depth: number, group: string | null) => {
    const on = chosen.has(place.written);
    const away = place.online === false;
    const note = place.here ? words.here : "";
    const rest = group && place.written.startsWith(group) ? place.written.slice(group.length).replace(/^[\s\-_\u00b7.]+/, "") : "";
    const title = [place.written, away ? words.offline : ""].filter(Boolean).join("　");
    return `<button type="button" class="focus-toggle${on ? " chosen" : ""}${away ? " offline" : ""}" data-act="toggle" data-machine="${escape(place.written)}" aria-pressed="${on}" style="--depth:${depth}" title="${escape(title)}"><span class="focus-mark">${glyph(place.form ?? "machine", 16)}</span><span class="focus-name">${escape(rest || place.written)}</span>${note ? `<span class="focus-note">${escape(note)}</span>` : ""}</button>`;
  };
  return holdings
    .map((holding) => {
      const loose = holding.loose.filter((place) => matches(place, null)).sort(byName);
      const groups = holding.groups
        .sort((a, b) => a.label.localeCompare(b.label, undefined, { numeric: true }))
        .map((group) => {
          const shown = group.places.filter((place) => matches(place, group.label)).sort(byName);
          if (!shown.length) return "";
          const key = `${holding.owner ?? ""}/${group.label}`;
          const held = group.places.filter((place) => chosen.has(place.written)).length;
          const open = !!query || (folds.get(key) ?? (group.places.length <= OPEN_GROUP || held > 0));
          return `<button type="button" class="focus-group${open ? " open" : ""}" data-act="fold" data-key="${escape(key)}" aria-expanded="${open}"><span class="focus-mark">${glyph("group", 16)}</span><span class="focus-name">${escape(group.label)}</span><span class="focus-count-of">${shown.length}</span>${held ? `<span class="focus-held-count">${held}</span>` : ""}<span class="focus-arrow" aria-hidden="true">${glyph("chevron", 14)}</span></button><div class="focus-fold${open ? "" : " shut"}" data-fold="${escape(key)}"${open ? "" : " inert"}><div class="focus-fold-inner">${shown.map((place) => row(place, 1, group.label)).join("")}</div></div>`;
        })
        .join("");
      if (!loose.length && !groups) return "";
      const title = holding.owner === null ? words.mine : words.theirs(holding.owner);
      return `<section class="focus-part"><h3 class="picker-title">${escape(title)}</h3>${loose.map((place) => row(place, 0, null)).join("")}${groups}</section>`;
    })
    .join("");
}

/** Opens or closes the group whose row is `head`, where it is drawn: its
 *  machines slide in or out; kept for the next time. */
export function foldChoice(root: HTMLElement, head: HTMLElement, folds: Map<string, boolean>): void {
  const key = head.dataset.key ?? "";
  const open = head.getAttribute("aria-expanded") !== "true";
  folds.set(key, open);
  keepFolds(folds);
  head.setAttribute("aria-expanded", String(open));
  head.classList.toggle("open", open);
  const body = [...root.querySelectorAll<HTMLElement>(".focus-fold")].find((one) => one.dataset.fold === key);
  body?.classList.toggle("shut", !open);
  if (body) body.inert = !open;
}
