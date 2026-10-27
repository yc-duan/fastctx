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
import { naturally } from "./machine-groups";
import { readOn } from "./read-on";

export interface MachineChoiceWords {
  here: string;
  offline: string;
  mine: string;
  theirs(owner: string): string;
  /** Over the other people's rows, where each is a row of their own. */
  others: string;
}

/** A group opens by itself up to this many machines. */
const OPEN_GROUP = 12;

/** Past this many other people, each is a row that opens onto their
 *  machines, drawn as it is opened: a network's administrator may hold
 *  thousands. One holding a machine chosen stands open. */
const PEOPLE_INLINE = 8;

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
  Number(b.here) - Number(a.here) || Number(b.online !== false) - Number(a.online !== false) || naturally(a.written, b.written);

/** The choice drawn, and how many rows each person's opens onto. */
export interface ChoiceDrawn {
  /** Empty where nothing holds what is looked for. */
  html: string;
  /** By a person's key: the rows of their machines and groups, drawn as the
   *  person is opened. */
  inside: Map<string, number>;
}

/** The machines as rows to choose by `data-act="toggle"`, groups to open by
 *  `data-act="fold"`. At most `reach` rows are drawn, a group's, a person's
 *  or a machine's alike; the rest wait under a mark at the end
 *  (`read-on.ts`). */
export function machineChoice(places: FocusPlace[], chosen: ReadonlySet<string>, query: string, folds: ReadonlyMap<string, boolean>, words: MachineChoiceWords, reach = Infinity): ChoiceDrawn {
  let left = reach;
  let cut = false;
  const take = () => {
    if (left <= 0) {
      cut = true;
      return false;
    }
    left -= 1;
    return true;
  };
  const holdings: Holding[] = [];
  const holder = new Map<string | null, Holding>();
  const grouped = new Map<string, Holding["groups"][number]>();
  for (const place of places) {
    const owner = place.owner ?? null;
    let holding = holder.get(owner);
    if (!holding) {
      holding = { owner, loose: [], groups: [] };
      holdings.push(holding);
      holder.set(owner, holding);
    }
    if (!place.group) holding.loose.push(place);
    else {
      const key = `${owner ?? ""}/${place.group}`;
      let group = grouped.get(key);
      if (!group) {
        group = { label: place.group, places: [] };
        holding.groups.push(group);
        grouped.set(key, group);
      }
      group.places.push(place);
    }
  }
  const rowed = holdings.filter((holding) => holding.owner !== null).length > PEOPLE_INLINE;
  holdings.sort((a, b) => (a.owner === null ? -1 : b.owner === null ? 1 : a.owner.localeCompare(b.owner)));
  const matches = (place: FocusPlace, group: string | null) => !query || place.written.toLowerCase().includes(query) || (!!group && group.toLowerCase().includes(query));
  const row = (place: FocusPlace, depth: number, group: string | null) => {
    if (!take()) return "";
    const on = chosen.has(place.written);
    const away = place.online === false;
    const note = place.here ? words.here : "";
    const rest = group && place.written.startsWith(group) ? place.written.slice(group.length).replace(/^[\s\-_\u00b7.]+/, "") : "";
    const title = [place.written, away ? words.offline : ""].filter(Boolean).join("　");
    return `<button type="button" class="focus-toggle${on ? " chosen" : ""}${away ? " offline" : ""}" data-act="toggle" data-machine="${escape(place.written)}" aria-pressed="${on}" style="--depth:${depth}" title="${escape(title)}"><span class="focus-mark">${glyph(place.form ?? "machine", 16)}</span><span class="focus-name">${escape(rest || place.written)}</span>${note ? `<span class="focus-note">${escape(note)}</span>` : ""}</button>`;
  };
  // One person's machines, `depth` steps in: those in no group, then
  // each group's, a row for the group and one for each of its machines.
  const loosely = (holding: Holding) => holding.loose.filter((place) => matches(place, null)).sort(byName);
  const groupsOf = (holding: Holding) =>
    holding.groups
      .sort((a, b) => naturally(a.label, b.label))
      .map((group) => ({ group, shown: group.places.filter((place) => matches(place, group.label)).sort(byName) }))
      .filter(({ shown }) => shown.length);
  const inside = (holding: Holding, depth: number) => {
    const loose = loosely(holding).map((place) => row(place, depth, null)).join("");
    const groups = groupsOf(holding)
      .map(({ group, shown }) => {
        if (!take()) return "";
        const key = `${holding.owner ?? ""}/${group.label}`;
        const held = group.places.filter((place) => chosen.has(place.written)).length;
        const open = !!query || (folds.get(key) ?? (group.places.length <= OPEN_GROUP || held > 0));
        const head = `<span class="focus-mark">${glyph("group", 16)}</span><span class="focus-name">${escape(group.label)}</span><span class="focus-count-of">${shown.length}</span>${held ? `<span class="focus-held-count">${held}</span>` : ""}`;
        return foldRow(key, open, head, shown.map((place) => row(place, depth + 1, group.label)).join(""), depth);
      })
      .join("");
    return loose + groups;
  };
  const counted = new Map<string, number>();
  const end = () => (cut ? readOn : "");
  const mine = holdings.filter((holding) => holding.owner === null || !rowed);
  const parts = mine
    .map((holding) => {
      const rows = inside(holding, 0);
      if (!rows) return "";
      const title = holding.owner === null ? words.mine : words.theirs(holding.owner);
      return `<section class="focus-part"><h3 class="picker-title">${escape(title)}</h3>${rows}</section>`;
    })
    .join("");
  if (!rowed) return { html: parts + end(), inside: counted };
  const people = holdings
    .filter((holding) => holding.owner !== null)
    .map((holding) => {
      const all = [...holding.loose, ...holding.groups.flatMap((group) => group.places)];
      const found = all.filter((place) => matches(place, place.group ?? null));
      if (!found.length) return "";
      const owner = holding.owner!;
      const key = `person:${owner}`;
      counted.set(key, loosely(holding).length + groupsOf(holding).reduce((sum, { shown }) => sum + 1 + shown.length, 0));
      if (!take()) return "";
      const held = all.filter((place) => chosen.has(place.written)).length;
      const open = !!query || (folds.get(key) ?? held > 0);
      const head = `<span class="focus-mark"><span class="focus-face">${escape([...owner][0]?.toUpperCase() ?? "")}</span></span><span class="focus-name">${escape(owner)}</span><span class="focus-count-of">${found.length}</span>${held ? `<span class="focus-held-count">${held}</span>` : ""}`;
      return foldRow(key, open, head, open ? inside(holding, 1) : "", 0);
    })
    .join("");
  const html = people ? `${parts}<section class="focus-part"><h3 class="picker-title">${escape(words.others)}</h3>${people}</section>` : parts;
  return { html: html + end(), inside: counted };
}

/** A row that opens and closes onto `inner`, `depth` steps in. */
function foldRow(key: string, open: boolean, head: string, inner: string, depth: number): string {
  const at = depth ? ` style="--depth:${depth}"` : "";
  return `<button type="button" class="focus-group${open ? " open" : ""}" data-act="fold" data-key="${escape(key)}" aria-expanded="${open}"${at}>${head}<span class="focus-arrow" aria-hidden="true">${glyph("chevron", 14)}</span></button><div class="focus-fold${open ? "" : " shut"}" data-fold="${escape(key)}"${open ? "" : " inert"}><div class="focus-fold-inner">${inner}</div></div>`;
}

/** Opens or closes the group or person whose row is `head`, where it is
 *  drawn: its machines slide in or out; kept for the next time. A person
 *  folded has nothing drawn under it: what `drawn` then gives is put in
 *  first. */
export function foldChoice(root: HTMLElement, head: HTMLElement, folds: Map<string, boolean>, drawn?: () => string): void {
  const key = head.dataset.key ?? "";
  const open = head.getAttribute("aria-expanded") !== "true";
  folds.set(key, open);
  keepFolds(folds);
  head.setAttribute("aria-expanded", String(open));
  head.classList.toggle("open", open);
  const body = [...root.querySelectorAll<HTMLElement>(".focus-fold")].find((one) => one.dataset.fold === key);
  const inner = body?.querySelector<HTMLElement>(":scope > .focus-fold-inner");
  if (open && drawn && inner && !inner.childElementCount) {
    const fresh = document.createElement("template");
    fresh.innerHTML = drawn();
    const filled = [...fresh.content.querySelectorAll<HTMLElement>(".focus-fold")].find((one) => one.dataset.fold === key)?.querySelector<HTMLElement>(":scope > .focus-fold-inner");
    if (filled) inner.replaceChildren(...filled.childNodes);
  }
  body?.classList.toggle("shut", !open);
  if (body) body.inert = !open;
}
