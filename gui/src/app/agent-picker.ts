// Choosing which agent a new conversation goes to (`world-gui.md`, 新对话):
// every login the user may begin with, as one table drawn as the machine
// list's tree. The user's own machines come first, then those shared with
// them and other members' an administrator may use, each person a row that
// opens onto their machines, then those whose agents assist them. A machine
// is its icon and name, grey while it is away; under it a row a login, the
// login's mark standing under the machine's name. Every login row has the
// same columns down the whole table: the agent, who is logged in, the plan,
// each usage window as a short bar with how much of it is spent, and why it
// cannot begin where it cannot. The chosen row is washed in clay. Above the
// table, where both agents are there, a choice of all or one of them; past
// ten logins a field narrows it to those whose machine, agent, account or
// plan holds what is typed (`new-talk.ts`).

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AgentFacts } from "../world/agents";
import { accountOf, windowsShort } from "./agent-homes";

export type PickerHold = "mine" | "shared" | "others" | "assist";

/** Which agents the table shows. */
export type PickerKind = "all" | "codex" | "claude";

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
  allAgents: string;
  noAgents: string;
  noMachine: string;
  token: string;
  onCredits: string;
  /** A usage window past the reset it was read before. */
  reset: string;
  /** A person's row: how many machines, how many logins. */
  personSub(machines: number, logins: number): string;
}

/** What the viewer set: which agents, and which people are open. */
export interface PickerView {
  kind: PickerKind;
  open(key: string, byDefault: boolean): boolean;
}

const HOLDS: PickerHold[] = ["mine", "shared", "others", "assist"];

/** Past this many logins the table can be narrowed by what is typed. */
export const FILTER_PAST = 10;

/** A person whose machines are shared with the user opens by itself up to
 *  this many logins; a member an administrator oversees waits closed. */
const OPEN_PERSON = 6;

const programName = (agent: string) => (agent === "codex" ? "Codex" : agent === "claude" ? "Claude" : agent);
const markOf = (agent: string): GlyphName => (agent === "claude" ? "claude" : "codex");

export function agentPicker(machines: PickerMachine[], words: PickerWords, view: PickerView): string {
  const body = HOLDS.map((hold) => {
    const held = machines.filter((machine) => machine.hold === hold);
    if (!held.length) return "";
    const people = hold === "shared" || hold === "others";
    const rows = people ? persons(held, hold === "shared", words, view) : held.map((machine) => machineRows(machine, 0, words)).join("");
    return `<div class="agent-hold"><h3 class="agent-group">${escape(words.holds[hold])}</h3>${rows}</div>`;
  }).join("");
  return `<div class="agent-picker" role="radiogroup" aria-label="${escape(words.agent)}">${body || `<p class="agent-none">${escape(words.noMachine)}</p>`}</div>`;
}

/** The choice of all agents or one, where the table holds both. */
export function agentKinds(machines: PickerMachine[], words: PickerWords, kind: PickerKind): string {
  const agents = new Set(machines.flatMap((machine) => machine.rows.map((row) => row.agent)));
  if (!(agents.has("codex") && agents.has("claude"))) return "";
  const choice = (value: PickerKind, label: string, mark?: GlyphName) =>
    `<button type="button" class="picker-kind${kind === value ? " chosen" : ""}" role="radio" aria-checked="${kind === value}" data-choice="kind" data-value="${value}">${mark ? glyph(mark, 14) : ""}<span>${escape(label)}</span></button>`;
  return `<div class="picker-kinds" role="radiogroup" aria-label="${escape(words.agent)}">${choice("all", words.allAgents)}${choice("codex", "Codex", "codex")}${choice("claude", "Claude", "claude")}</div>`;
}

/** Other people's machines: a row a person, opening onto theirs. A person
 *  holding the login chosen is open, so the choice is in sight. */
function persons(held: PickerMachine[], shared: boolean, words: PickerWords, view: PickerView): string {
  const owners = [...new Set(held.map((machine) => machine.owner ?? ""))];
  return owners
    .map((owner) => {
      const theirs = held.filter((machine) => (machine.owner ?? "") === owner);
      const logins = theirs.reduce((sum, machine) => sum + machine.rows.length, 0);
      const chosen = theirs.some((machine) => machine.rows.some((row) => row.chosen));
      const key = `person:${owner}`;
      const open = view.open(key, chosen || (shared && logins <= OPEN_PERSON));
      const face = `<span class="agent-face">${escape([...owner][0]?.toUpperCase() ?? "")}</span>`;
      const head = `<button type="button" class="agent-person${open ? " open" : ""}" data-choice="fold" data-value="${escape(key)}" aria-expanded="${open}">${twisty(true)}<span class="agent-icon" aria-hidden="true">${face}</span><span class="agent-machine-name">${escape(owner)}</span><span class="agent-machine-note">${escape(words.personSub(theirs.length, logins))}</span></button>`;
      const inside = theirs.map((machine) => machineRows(machine, 1, words)).join("");
      return `<div class="agent-people" data-find="${escape(owner.toLowerCase())}">${head}<div class="agent-people-body"${open ? "" : " data-folded"}>${inside}</div></div>`;
    })
    .join("");
}

/** The twisty of a row that opens, or its room on one that does not. */
function twisty(folds: boolean): string {
  return `<span class="agent-twisty" aria-hidden="true">${folds ? glyph("chevron", 14) : ""}</span>`;
}

function machineRows(machine: PickerMachine, depth: number, words: PickerWords): string {
  const state = machine.online === false ? " offline" : machine.online === true ? " online" : "";
  const at = depth ? ` style="--depth:${depth}"` : "";
  const notes = machine.notes.map((note) => `<span class="agent-machine-note">${escape(note)}</span>`).join("");
  const head = `<div class="agent-machine${state}"${at}>${twisty(false)}<span class="agent-icon" aria-hidden="true">${glyph(machine.form, 15)}</span><span class="agent-machine-name">${escape(machine.written)}</span>${notes}</div>`;
  const host = (inner: string) => `<div class="agent-host" data-find="${escape(machine.written.toLowerCase())}">${inner}</div>`;
  if (machine.online === false) return host(head);
  const rows = machine.rows.length ? machine.rows.map((row) => rowHtml(row, depth, words)).join("") : `<p class="agent-none quiet"${at}>${escape(words.noAgents)}</p>`;
  return host(head + rows);
}

function rowHtml(row: PickerRow, depth: number, words: PickerWords): string {
  const name = programName(row.agent);
  const facts = row.facts;
  const account = facts ? accountOf(facts) : { email: null, plan: null };
  const token = facts?.login.kind === "known" && facts.login.value.kind === "logged_in" && facts.login.value.method === "oauth_token";
  const plan = account.plan ?? (token ? words.token : null);
  const now = Date.now();
  const limits = row.note ? "" : limitsHtml(facts, words, now);
  const credits = !row.note && onCredits(facts, now) ? words.onCredits : null;
  const note = row.note ?? row.caution ?? credits;
  const windows = limits ? windowsShort(facts?.limits, now).map((window) => `${window.label} ${window.reset ? words.reset : window.percent}`) : [];
  const said = [name, row.who, plan, windows.join("　"), note].filter(Boolean).join("　");
  const kind = row.note ? "" : row.caution ? " caution" : credits ? " credits" : "";
  const find = [name, row.who, plan].filter(Boolean).join(" ").toLowerCase();
  const at = depth ? ` style="--depth:${depth}"` : "";
  return `<button type="button" class="agent-row${row.chosen ? " chosen" : ""}" role="radio" aria-checked="${row.chosen}" aria-label="${escape(said)}" title="${escape(said)}" data-choice="pick" data-value="${escape(row.key)}" data-agent="${escape(row.agent)}" data-find="${escape(find)}"${at}${row.startable ? "" : " disabled"}>
    <span class="agent-row-mark">${glyph(markOf(row.agent), 15)}</span>
    <span class="agent-row-agent">${escape(name)}</span>
    <span class="agent-row-who">${escape(row.who ?? "")}</span>
    <span class="agent-row-plan">${plan ? `<span class="tag">${escape(plan)}</span>` : ""}</span>
    <span class="agent-row-limits">${limits}</span>
    <span class="agent-row-note${kind}">${note ? escape(note) : ""}</span>
  </button>`;
}

/** Narrows the table drawn in `root` to one agent's logins, and to what
 *  holds `typed`: a machine whose name holds it keeps all its logins;
 *  otherwise the logins whose agent, account or plan holds it, under their
 *  machines. A person whose name holds it keeps all theirs. A machine with
 *  no login left goes, unless nothing is narrowed; a person or a group with
 *  none left goes. While something is typed, closed people open onto what
 *  matches. */
export function narrowPicker(root: HTMLElement, typed: string, kind: PickerKind = "all"): void {
  const query = typed.trim().toLowerCase();
  const narrowing = !!query || kind !== "all";
  root.querySelector(".agent-picker")?.classList.toggle("narrowing", !!query);
  for (const hold of root.querySelectorAll<HTMLElement>(".agent-hold")) {
    let any = false;
    for (const people of hold.querySelectorAll<HTMLElement>(".agent-people")) people.hidden = false;
    for (const host of hold.querySelectorAll<HTMLElement>(".agent-host")) {
      const person = host.closest<HTMLElement>(".agent-people");
      const named = !query || (host.dataset.find ?? "").includes(query) || (!!person && (person.dataset.find ?? "").includes(query));
      let rows = false;
      for (const row of host.querySelectorAll<HTMLElement>(".agent-row")) {
        const shown = (kind === "all" || row.dataset.agent === kind) && (named || (row.dataset.find ?? "").includes(query));
        row.hidden = !shown;
        rows ||= shown;
      }
      for (const none of host.querySelectorAll<HTMLElement>(".agent-none")) none.hidden = narrowing;
      host.hidden = narrowing ? !rows : false;
      any ||= !host.hidden;
    }
    for (const people of hold.querySelectorAll<HTMLElement>(".agent-people")) {
      people.hidden = narrowing && !people.querySelector(".agent-host:not([hidden])");
    }
    hold.hidden = !any;
  }
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
