// Choosing which agent a new conversation goes to (`world-gui.md`, 新对话):
// every login of one agent the user may begin with, Codex or Claude, as one
// table. The user's own machines come first, then those shared with them and
// other members' an administrator may use, each person a row that opens onto
// their machines, then those whose agents assist them. A machine is a row
// that opens and closes onto its logins, as a project does in the sidebar:
// its icon and name, grey while it is away, and an arrow at the end. Its
// logins stand a small step in, each the agent's mark, who is logged in,
// the plan, each usage window as a short bar with how much of it is spent,
// and why it cannot begin where it cannot, on the same columns down the
// whole table. The chosen row is washed in clay. Above the table, a field
// narrows it to the machines and logins that hold what is typed
// (`new-talk.ts`). A table of thousands is drawn as far as it is read.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AgentFacts } from "../world/agents";
import { accountOf, windowsShort } from "./agent-homes";
import { readOn } from "./read-on";

export type PickerHold = "mine" | "shared" | "others" | "assist";

/** Which agent's logins the table shows. */
export type PickerKind = "codex" | "claude";

/** One machine as the picker shows it. */
export interface PickerMachine {
  hold: PickerHold;
  written: string;
  /** Whose it is, where it is another person's. */
  owner: string | null;
  /** What kind of computer it is, for its icon. */
  form: "desktop" | "laptop" | "server";
  online: boolean | null;
  /** What follows its name: this computer, offline, whom it assists from. */
  notes: string[];
  rows: PickerRow[];
}

/** One login on a machine. */
export interface PickerRow {
  key: string;
  agent: string;
  facts: AgentFacts | null;
  /** Who is logged in, as the row names it. */
  who: string | null;
  /** Why it cannot begin, or what is not known of it yet. */
  note: string | null;
  /** What it lacks though it can begin: the user's consent for Claude. */
  caution: string | null;
  startable: boolean;
  chosen: boolean;
}

export interface PickerWords {
  holds: Record<PickerHold, string>;
  agent: string;
  /** Where none of the user's machines has a login of this agent. */
  noneOf(agent: string): string;
  noMachine: string;
  token: string;
  onCredits: string;
  /** A usage window past the reset it was read before. */
  reset: string;
  /** A person's row: how many machines, how many logins. */
  personSub(machines: number, logins: number): string;
}

/** What the viewer set: which agent, which machines and people are open,
 *  and what is typed to narrow the table to, lower-cased. */
export interface PickerView {
  kind: PickerKind;
  open(key: string, byDefault: boolean): boolean;
  query?: string;
  /** At most how many rows are drawn, a person's, a machine's or a login's
   *  alike; the rest wait under a mark at the end (`read-on.ts`). Every row
   *  when absent. */
  reach?: number;
}

/** The table drawn, and how many rows each person's opens onto. */
export interface PickerDrawn {
  html: string;
  /** By a person's key: the rows of their machines and logins, drawn as
   *  the person is opened. */
  inside: Map<string, number>;
}

/** The rows a drawing may still take, and whether any were left out. */
class Budget {
  private left: number;
  cut = false;

  constructor(reach: number | undefined) {
    this.left = reach ?? Infinity;
  }

  /** Takes the next row: whether it is drawn. */
  take(): boolean {
    if (this.left <= 0) {
      this.cut = true;
      return false;
    }
    this.left -= 1;
    return true;
  }
}

const HOLDS: PickerHold[] = ["mine", "shared", "others", "assist"];

/** A person whose machines are shared with the user opens by itself up to
 *  this many logins; a member an administrator oversees waits closed. */
const OPEN_PERSON = 6;

export const programName = (agent: string) => (agent === "codex" ? "Codex" : agent === "claude" ? "Claude" : agent);
const markOf = (agent: string): GlyphName => (agent === "claude" ? "claude" : "codex");

/** The key a machine's row opens and closes by. */
export const machineKey = (written: string) => `machine:${written}`;

export function agentPicker(machines: PickerMachine[], words: PickerWords, view: PickerView): PickerDrawn {
  // Only this agent's logins; a machine with none of them is not listed.
  const kept = machines
    .map((machine) => ({ ...machine, rows: machine.rows.filter((row) => row.agent === view.kind) }))
    .filter((machine) => machine.rows.length);
  // Narrowed, a machine whose name or owner holds what is typed keeps all
  // its logins, any other the logins whose account or plan holds it; all
  // of it open.
  const query = view.query ?? "";
  const shown = query
    ? kept.flatMap((machine) => {
        const named = machine.written.toLowerCase().includes(query) || (machine.owner ?? "").toLowerCase().includes(query);
        const rows = named ? machine.rows : machine.rows.filter((row) => rowFind(row, words).includes(query));
        return rows.length ? [{ ...machine, rows }] : [];
      })
    : kept;
  if (query) view = { ...view, open: () => true };
  const budget = new Budget(view.reach);
  const inside = new Map<string, number>();
  const body = HOLDS.map((hold) => {
    const held = shown.filter((machine) => machine.hold === hold);
    if (!held.length) return "";
    const people = hold === "shared" || hold === "others";
    const rows = people ? persons(held, hold === "shared", words, view, budget, inside) : held.map((machine) => machineRows(machine, 0, words, view, budget)).join("");
    return rows ? `<div class="agent-hold"><h3 class="agent-group">${escape(words.holds[hold])}</h3>${rows}</div>` : "";
  }).join("");
  const none = machines.length ? words.noneOf(programName(view.kind)) : words.noMachine;
  const more = budget.cut ? readOn : "";
  return { html: `<div class="agent-picker" role="radiogroup" aria-label="${escape(words.agent)}">${body || `<p class="agent-none">${escape(none)}</p>`}${more}</div>`, inside };
}

/** The choice of Codex or Claude, over the table. */
export function agentKinds(words: PickerWords, kind: PickerKind): string {
  const choice = (value: PickerKind) =>
    `<button type="button" class="segment picker-kind${kind === value ? " chosen" : ""}" role="radio" aria-checked="${kind === value}" data-choice="kind" data-value="${value}">${glyph(markOf(value), 14)}<span>${programName(value)}</span></button>`;
  return `<div class="segmented picker-kinds" role="radiogroup" aria-label="${escape(words.agent)}">${choice("codex")}${choice("claude")}</div>`;
}

/** A row that opens and closes what is under it, and what is under it. */
function fold(key: string, open: boolean, head: string, inside: string, depth: number, className: string): string {
  const at = depth ? ` style="--depth:${depth}"` : "";
  return `<button type="button" class="${className}${open ? " open" : ""}" data-choice="fold" data-value="${escape(key)}" aria-expanded="${open}"${at}>${head}<span class="agent-arrow" aria-hidden="true">${glyph("chevron", 14)}</span></button><div class="agent-fold${open ? "" : " shut"}" data-fold="${escape(key)}"${open ? "" : " inert"}><div class="agent-fold-inner">${inside}</div></div>`;
}

/** Other people's machines: a row a person, opening onto theirs. A person
 *  holding the login chosen is open, so the choice is in sight. */
function persons(held: PickerMachine[], shared: boolean, words: PickerWords, view: PickerView, budget: Budget, inside: Map<string, number>): string {
  const owners = new Map<string, PickerMachine[]>();
  for (const machine of held) {
    const theirs = owners.get(machine.owner ?? "");
    if (theirs) theirs.push(machine);
    else owners.set(machine.owner ?? "", [machine]);
  }
  return [...owners]
    .map(([owner, theirs]) => {
      const key = `person:${owner}`;
      // A machine's row, and its logins' where it is there to begin on.
      inside.set(key, theirs.reduce((sum, machine) => sum + 1 + (machine.online === false ? 0 : machine.rows.length), 0));
      if (!budget.take()) return "";
      const logins = theirs.reduce((sum, machine) => sum + machine.rows.length, 0);
      const chosen = theirs.some((machine) => machine.rows.some((row) => row.chosen));
      const open = view.open(key, chosen || (shared && logins <= OPEN_PERSON));
      const head = `<span class="agent-icon" aria-hidden="true"><span class="agent-face">${escape([...owner][0]?.toUpperCase() ?? "")}</span></span><span class="agent-machine-name">${escape(owner)}</span><span class="agent-machine-note">${escape(words.personSub(theirs.length, logins))}</span>`;
      // Folded, a person's machines are drawn as they are opened: a network's
      // administrator may hold thousands.
      const rows = open ? theirs.map((machine) => machineRows(machine, 1, words, view, budget)).join("") : "";
      return `<div class="agent-people">${fold(key, open, head, rows, 0, "agent-person")}</div>`;
    })
    .join("");
}

function machineRows(machine: PickerMachine, depth: number, words: PickerWords, view: PickerView, budget: Budget): string {
  if (!budget.take()) return "";
  const state = machine.online === false ? " offline" : machine.online === true ? " online" : "";
  const notes = machine.notes.map((note) => `<span class="agent-machine-note">${escape(note)}</span>`).join("");
  const head = `<span class="agent-icon" aria-hidden="true">${glyph(machine.form, 15)}</span><span class="agent-machine-name">${escape(machine.written)}</span>${notes}`;
  const host = (inner: string) => `<div class="agent-host">${inner}</div>`;
  // Away, it is its name alone: nothing under it can begin.
  if (machine.online === false) {
    const at = depth ? ` style="--depth:${depth}"` : "";
    return host(`<div class="agent-machine${state}"${at}>${head}</div>`);
  }
  const chosen = machine.rows.some((row) => row.chosen);
  const key = machineKey(machine.written);
  const open = view.open(key, true) || chosen;
  const rows = machine.rows.map((row) => (budget.take() ? rowHtml(row, depth + 1, words) : "")).join("");
  return host(fold(key, open, head, rows, depth, `agent-machine${state}`));
}

/** A login's plan, as its row says it. */
function planOf(row: PickerRow, words: PickerWords): string | null {
  const facts = row.facts;
  const account = facts ? accountOf(facts) : { email: null, plan: null };
  const token = facts?.login.kind === "known" && facts.login.value.kind === "logged_in" && facts.login.value.method === "oauth_token";
  return account.plan ?? (token ? words.token : null);
}

/** What a login is found by: who is logged in, and the plan. */
function rowFind(row: PickerRow, words: PickerWords): string {
  return [row.who, planOf(row, words)].filter(Boolean).join(" ").toLowerCase();
}

function rowHtml(row: PickerRow, depth: number, words: PickerWords): string {
  const name = programName(row.agent);
  const facts = row.facts;
  const plan = planOf(row, words);
  const now = Date.now();
  const limits = row.note ? "" : limitsHtml(facts, words, now);
  const credits = !row.note && onCredits(facts, now) ? words.onCredits : null;
  const note = row.note ?? row.caution ?? credits;
  const windows = limits ? windowsShort(facts?.limits, now).map((window) => `${window.label} ${window.reset ? words.reset : window.percent}`) : [];
  const who = row.who ?? name;
  const said = [name, who === name ? null : who, plan, windows.join("　"), note].filter(Boolean).join("　");
  const kind = row.note ? "" : row.caution ? " caution" : credits ? " credits" : "";
  return `<button type="button" class="agent-row${row.chosen ? " chosen" : ""}" role="radio" aria-checked="${row.chosen}" aria-label="${escape(said)}" title="${escape(said)}" data-choice="pick" data-value="${escape(row.key)}" style="--depth:${depth}"${row.startable ? "" : " disabled"}>
    <span class="agent-row-lead"><span class="agent-row-mark">${glyph(markOf(row.agent), 15)}</span><span class="agent-row-who">${escape(who)}</span></span>
    <span class="agent-row-plan">${plan ? escape(plan) : ""}</span>
    <span class="agent-row-limits">${limits}</span>
    <span class="agent-row-note${kind}">${note ? escape(note) : ""}</span>
  </button>`;
}

/** Opens or closes one row's machines or logins where it is drawn: what is
 *  under it slides in or out. */
export function foldInPicker(root: HTMLElement, key: string, open: boolean): void {
  const head = [...root.querySelectorAll<HTMLElement>("[data-choice='fold']")].find((one) => one.dataset.value === key);
  const body = [...root.querySelectorAll<HTMLElement>(".agent-fold")].find((one) => one.dataset.fold === key);
  if (!head || !body) return;
  head.setAttribute("aria-expanded", String(open));
  head.classList.toggle("open", open);
  body.classList.toggle("shut", !open);
  body.inert = !open && !root.querySelector(".agent-picker.narrowing");
}

/** Each usage window a short bar with how much of it is spent; one past
 *  its reset an empty bar that says so. */
function limitsHtml(facts: AgentFacts | null, words: PickerWords, now: number): string {
  return windowsShort(facts?.limits, now)
    .map((window) => {
      const width = window.reset ? 0 : Math.min(100, parseFloat(window.percent) || 0);
      return `<span class="agent-limit${window.level ? ` ${window.level}` : ""}"><span class="agent-limit-name">${escape(window.label)}</span><span class="agent-limit-track"><i style="width:${width}%"></i></span><span class="agent-limit-used">${escape(window.reset ? words.reset : window.percent)}</span></span>`;
    })
    .join("");
}

/** Whether a login goes on with credits: a window of it is spent and it
 *  has them. */
function onCredits(facts: AgentFacts | null, now: number): boolean {
  return windowsShort(facts?.limits, now).some((window) => window.level === "spent") && facts?.limits?.kind === "known" && !!facts.limits.value.credits;
}
