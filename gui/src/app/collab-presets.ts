// The presets of model collaboration as its page keeps them
// (`world-colleagues.md`): those turned on are the callees every release
// offers, those turned off are kept beside them, and one of the World's the
// account has none of yet is shown off until it is turned on or rewritten.
// Each change here makes the settings a client that knows presets writes:
// every callee says its preset.

import type { AgentModel } from "../world/agents";
import { AT_ONCE_DEFAULT, AT_ONCE_IN_ALL_DEFAULT, WORLD_KINDS, kindOf, type AgentLogin, type AtOnce, type Callee, type Collab, type CollabView, type PresetKind } from "../world/collab";

/** A preset as its row shows it. */
export interface Shown {
  callee: Callee;
  on: boolean;
  /** One of the World's the account keeps none of yet. */
  offered: boolean;
}

/** A login a preset may run in, Codex's or Claude Code's. */
export interface CalleeLogin {
  login: AgentLogin;
  /** The agent, the machine, then whose login it is. */
  label: string;
  models: AgentModel[];
}

/** The login a new preset runs in unless another is chosen: GPT's first,
 *  as it always was, else Claude's. */
export const defaultLogin = (logins: CalleeLogin[]): CalleeLogin | null =>
  logins.find((one) => one.login.agent === "codex") ?? logins[0] ?? null;

const ORDER: PresetKind[] = ["review", "scout", "own"];

/** What a scout runs with unless told otherwise: quick, and many at once. */
const SCOUT_EFFORT = "low";
const SCOUT_AT_ONCE = 8;

const kept = (collab: Collab): Callee[] => [...collab.callees, ...(collab.shelved ?? [])];

/** The first name of `kind` no preset of the account has. */
export function freeName(collab: Collab, kind: PresetKind): string {
  const base = kind === "review" ? "gpt" : kind === "scout" ? "scout" : "helper";
  const taken = new Set(kept(collab).map((callee) => callee.name));
  for (let n = 1; ; n += 1) {
    const name = n === 1 ? base : `${base}-${n}`;
    if (!taken.has(name)) return name;
  }
}

/** A new preset of `kind`, in `codex` where there is one: a scout quick and
 *  many at once, without the screen. */
export function fresh(collab: Collab, kind: PresetKind, codex: CalleeLogin | null): Callee {
  const scout = kind === "scout";
  const model = codex?.models.find((one) => one.is_default) ?? codex?.models[0];
  const quick = scout && model?.reasoning_efforts.some((one) => one.effort === SCOUT_EFFORT);
  return {
    name: freeName(collab, kind),
    use_when: "",
    login: codex?.login ?? { machine: "", agent: "codex", home: null },
    model: null,
    effort: quick ? SCOUT_EFFORT : null,
    screen: !scout,
    ...(scout ? { at_once: { most: SCOUT_AT_ONCE } } : {}),
    preset: { kind },
  };
}

/** The account's presets, the World's first, each kind by name: those it
 *  keeps, and where presets are kept, the World's it has none of. */
export function shownPresets(view: CollabView, codex: CalleeLogin | null): Shown[] {
  const { collab } = view;
  const shown: Shown[] = [
    ...collab.callees.map((callee) => ({ callee, on: true, offered: false })),
    ...(collab.shelved ?? []).map((callee) => ({ callee, on: false, offered: false })),
  ];
  if (view.keeps_presets) {
    for (const kind of WORLD_KINDS) {
      if (!shown.some((one) => kindOf(one.callee) === kind)) shown.push({ callee: fresh(collab, kind, codex), on: false, offered: true });
    }
  }
  return shown.sort((a, b) => ORDER.indexOf(kindOf(a.callee)) - ORDER.indexOf(kindOf(b.callee)) || a.callee.name.localeCompare(b.callee.name));
}

/** The settings as a client that knows presets writes them: every callee
 *  says its preset, one written before them taking the account's text for
 *  callers as its own; machines from before presets go on giving callers
 *  the first reviewer's. */
function said(collab: Collab): Collab {
  const preset = (callee: Callee): Callee =>
    callee.preset ? callee : { ...callee, preset: { kind: "review", ...(collab.prompt !== null ? { prompt: collab.prompt } : {}) } };
  const callees = collab.callees.map(preset);
  const first = callees.find((callee) => kindOf(callee) === "review");
  return { ...collab, callees, shelved: (collab.shelved ?? []).map(preset), prompt: first?.preset?.prompt ?? null };
}

/** `collab` with the preset called `previous` rewritten as `callee`, or
 *  `callee` added, turned on or kept off. Where presets are not kept
 *  (`legacy`), it is a callee as before them. */
export function withSaved(collab: Collab, previous: string | null, callee: Callee, on: boolean, legacy: boolean): Collab {
  if (legacy) {
    const callees = collab.callees.some((one) => one.name === previous) ? collab.callees.map((one) => (one.name === previous ? callee : one)) : [...collab.callees, callee];
    return { ...collab, callees };
  }
  const now = said(collab);
  const place = (list: Callee[], here: boolean): Callee[] => {
    const at = list.findIndex((one) => one.name === previous);
    const rest = list.filter((one) => one.name !== previous);
    if (!here) return rest;
    // Rewritten where it stood; turned on or off, last among the others.
    return at < 0 ? [...rest, callee] : [...list.slice(0, at), callee, ...list.slice(at + 1)];
  };
  return said({ ...now, callees: place(now.callees, on), shelved: place(now.shelved ?? [], !on) });
}

/** `collab` without the preset called `name`. */
export function withRemoved(collab: Collab, name: string, legacy: boolean): Collab {
  if (legacy) return { ...collab, callees: collab.callees.filter((one) => one.name !== name) };
  const now = said(collab);
  return said({ ...now, callees: now.callees.filter((one) => one.name !== name), shelved: (now.shelved ?? []).filter((one) => one.name !== name) });
}

/** How many of a preset's colleagues work at once for one conversation;
 *  none for no limit. */
export const atOnceOf = (callee: Callee): number | null => (callee.at_once === "any" ? null : (callee.at_once?.most ?? AT_ONCE_DEFAULT));

/** How many of every preset's together do. */
export const atOnceInAll = (collab: Collab): number | null => (collab.at_once === "any" ? null : (collab.at_once?.most ?? AT_ONCE_IN_ALL_DEFAULT));

/** The limits offered, before no limit; one set elsewhere that is no step
 *  here stands among them in its place. */
const AT_ONCE_STEPS = [1, 2, 3, 4, 6, 8, 12, 16];

/** The choices of a limit, as a row of them is drawn: each with what it is
 *  kept as, `any` for no limit. */
export function atOnceChoices(most: number | null, noLimit: string): { chosen: string; choices: [string, string][] } {
  const counts = [...new Set([...AT_ONCE_STEPS, ...(most === null ? [] : [most])])].sort((a, b) => a - b);
  return { chosen: most === null ? "any" : String(most), choices: [...counts.map((count): [string, string] => [String(count), String(count)]), ["any", noLimit]] };
}

/** A choice of `atOnceChoices`, as the settings keep it. */
export const atOnceSaid = (value: string): AtOnce => (value === "any" ? "any" : { most: Number(value) });
