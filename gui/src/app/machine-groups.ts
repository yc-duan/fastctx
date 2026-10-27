// The groups of machines (`world-network.md`, 机器的分组): an owner puts some
// of their machines in a group, and the drawing, the list and the models
// shown them show them together under its name. A group is that and
// nothing more: it reaches no machine and changes no one's rights. Who may
// group a machine is who may rename it: its owner, and the network's
// administrator for a member's.

import { escape } from "../conversation/prose";
import { loginsOf, type AccountChange, type AccountName, type Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { holds, isSuper, sameLogin, type Machine } from "./reach";
import { indexOf } from "./world-index";

export interface GroupWords {
  /** The action beside a machine, and the title of its card. */
  group: string;
  groupTitle(machine: string): string;
  /** Several framed together. */
  groupMany(count: number): string;
  noGroup: string;
  /** The field that names a new group, and what is written in it. */
  newGroup: string;
  groupName: string;
  groupGo: string;
  cancel: string;
  /** A group's "⋯" on its box: its name changed, or the group ended. */
  more(group: string): string;
  renameGroup: string;
  renameGroupTitle(group: string): string;
  endGroup: string;
  /** Said in the drawing's corner, where the change can be undone. */
  movedOne(machine: string, group: string | null): string;
  movedMany(count: number, group: string | null): string;
  renamedGroup(from: string, to: string): string;
  endedGroup(group: string): string;
  /** The rule a group's name keeps. */
  groupNameLong: string;
  groupNameEmpty: string;
}

/** What groups `machine` from this device: the login that may, the owner
 *  whose groups they are, the group it is in, and that owner's groups. */
export interface Grouping {
  login: Login;
  owner: AccountName;
  now: string | null;
  groups: string[];
}

/** How `machine` is grouped from this device, where it may be: by a login
 *  that holds it or administers its hub, on a hub that keeps groups, as the
 *  hub refuses the administrator a visitor's machines. On `hub`, where one
 *  is named. */
export function groupingOf(machine: Machine, world: WorldNow, hub?: string): Grouping | null {
  const index = indexOf(world);
  for (const login of loginsOf(world.accounts)) {
    if (hub !== undefined && login.hub !== hub) continue;
    const seen = index.hubs.get(login.hub);
    if (!seen?.hub.groups || !machine.on.some((on) => on.hub === login.hub)) continue;
    // The computer by the name it goes by or had (`names`).
    const member = seen.computer.get(machine.name);
    const owner = member?.home;
    if (!member || !owner) continue;
    const own = owner === login.account || holds(machine, login, world);
    const visitor = seen.accounts.get(owner)?.level === "visitor";
    if (!own && !(isSuper(world, login) && !visitor)) continue;
    return { login, owner, now: member.group ?? null, groups: groupsOf(world, login.hub, owner) };
  }
  return null;
}

/** The one way `machines` are grouped together: by the same login, for the
 *  same owner; none where any of them may not be, or they are not one
 *  owner's. */
export function groupingTogether(machines: Machine[], world: WorldNow): Grouping[] | null {
  const each = machines.map((machine) => groupingOf(machine, world));
  const first = each[0];
  if (!first || !each.every((one) => one && sameLogin(one.login, first.login) && one.owner === first.owner)) return null;
  return each as Grouping[];
}

/** The groups `owner` has on `hub`, by name. */
export function groupsOf(world: WorldNow, hub: string, owner: AccountName): string[] {
  return [...(indexOf(world).hubs.get(hub)?.groups.get(owner) ?? [])];
}

/** A group's name as given, made plain the way the hub makes it: spaces at
 *  its ends dropped and those within it made one; the reason when it will
 *  not do. */
export function groupNameOf(text: string, words: GroupWords): { name: string } | { wrong: string } {
  const name = text.split(/\s+/).filter(Boolean).join(" ");
  if (!name) return { wrong: words.groupNameEmpty };
  if ([...name].length > 32) return { wrong: words.groupNameLong };
  return { name };
}

/** The card's form: no group and each of the owner's groups as pills, and
 *  a field below that names a new one. The one it is in now is chosen; for
 *  several in different groups, none is, and the card waits for a choice. */
export function groupForm(words: GroupWords, title: string, groups: string[], now: string | null | undefined): string {
  const choice = (attributes: string, label: string, checked: boolean) =>
    `<label class="choice map-card-choice"><input type="radio" name="group" ${attributes}${checked ? " checked" : ""}><span>${escape(label)}</span></label>`;
  return `<p class="map-card-title">${escape(title)}</p>
    <div class="choice-options group-choices" role="radiogroup" aria-label="${escape(words.group)}">
      ${choice("data-none", words.noGroup, now === null)}
      ${groups.map((group) => choice(`value="${escape(group)}"`, group, group === now)).join("")}
    </div>
    <label class="field group-new"><span class="field-label">${escape(words.newGroup)}</span><input name="new" maxlength="40" placeholder="${escape(words.groupName)}" autocomplete="off" spellcheck="false"></label>
    <p class="map-card-wrong" role="alert" hidden></p>
    <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit"${now === undefined ? " disabled" : ""}>${escape(words.groupGo)}</button></footer>`;
}

/** The group the card's form chose: a new one by its name where one is
 *  written, else the pill chosen, null for none; the reason when a new
 *  one's name will not do. */
export function chosenGroup(card: HTMLElement, words: GroupWords): { group: string | null } | { wrong: string } {
  const typed = card.querySelector<HTMLInputElement>("input[name=new]")?.value ?? "";
  if (typed.trim()) {
    const named = groupNameOf(typed, words);
    return "name" in named ? { group: named.name } : named;
  }
  const picked = card.querySelector<HTMLInputElement>("input[name=group]:checked");
  return { group: !picked || picked.dataset.none !== undefined ? null : picked.value };
}

/** A name written for a new group stands in the pills' stead, and a pill
 *  chosen empties it; the card goes once something is chosen. */
export function wireGroupForm(card: HTMLElement): void {
  const field = card.querySelector<HTMLInputElement>("input[name=new]");
  const radios = [...card.querySelectorAll<HTMLInputElement>("input[name=group]")];
  const go = card.querySelector<HTMLButtonElement>(".map-card-go");
  if (!field || !go) return;
  const ready = () => {
    go.disabled = !field.value.trim() && !radios.some((radio) => radio.checked);
  };
  field.addEventListener("input", () => {
    if (field.value.trim()) for (const radio of radios) radio.checked = false;
    ready();
  });
  for (const radio of radios)
    radio.addEventListener("change", () => {
      field.value = "";
      ready();
    });
}

/** The card that renames a group: its name in a field to change. */
export function groupRenameForm(words: GroupWords, group: string): string {
  return `<p class="map-card-title">${escape(words.renameGroupTitle(group))}</p>
    <label class="field group-rename"><input name="name" maxlength="40" value="${escape(group)}" aria-label="${escape(words.groupName)}" autocomplete="off" spellcheck="false"></label>
    <p class="map-card-wrong" role="alert" hidden></p>
    <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.groupGo)}</button></footer>`;
}

/** The key of a group's "⋯" on its box, by the hub and the box's kind
 *  (`mine/lab A`). */
export const groupMoreKey = (hub: string, kind: string) => `gm:${hub}:${kind}`;

/** The hub and box kind a group's "⋯" stands for; a hub's id has no colon. */
export function groupMoreOf(key: string): { hub: string; kind: string; group: string } | null {
  const found = key.match(/^gm:([^:]+):(.+)$/);
  const group = found ? boxParts(found[2]!).group : null;
  return found && group !== null ? { hub: found[1]!, kind: found[2]!, group } : null;
}

/** A box key's parts: whose box it is (`mine`, `of:bo`, `assist:bo`), and
 *  the group it holds of theirs, when it does (`mine/lab A`). An account's
 *  name has no slash, so the first one parts them. */
export function boxParts(key: string): { owner: string; group: string | null } {
  const at = key.indexOf("/");
  return at < 0 ? { owner: key, group: null } : { owner: key.slice(0, at), group: key.slice(at + 1) };
}

/** Two names compared as they are read, a number in them by its value: pc2
 *  before pc10. One collator for every comparison; `localeCompare` with its
 *  options makes one each time, which costs more than a sort of thousands. */
export const naturally = new Intl.Collator(undefined, { numeric: true }).compare;

/** The order of two boxes under a hub: by whose they are, as `rank` says
 *  and then by name; each owner's own box before their groups, and the
 *  groups by name. */
export function compareBoxes(a: string, b: string, rank: (owner: string) => number): number {
  const [x, y] = [boxParts(a), boxParts(b)];
  if (rank(x.owner) !== rank(y.owner)) return rank(x.owner) - rank(y.owner);
  if (x.owner !== y.owner) return x.owner < y.owner ? -1 : 1;
  if (x.group === null || y.group === null) return x.group === y.group ? 0 : x.group === null ? -1 : 1;
  return naturally(x.group, y.group);
}

/** The change that puts `machine` in `group`. */
export function groupChange(machine: string, group: string | null): AccountChange {
  return { change: "group", machine, group };
}
