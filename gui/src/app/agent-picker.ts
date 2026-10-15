// Choosing which agent a new conversation goes to (`world-gui.md`, 新对话):
// every login the user may begin with, as one table. The machines stand in
// groups as the user holds them, their own first, then those shared with
// them, other members' an administrator may use, and those whose agents
// assist them; under each machine a row a login, and every row has the
// same columns down the whole list: the agent, who is logged in, the plan,
// each usage window as a short bar with how much of it is spent, and why
// it cannot begin where it cannot. The chosen row is washed in clay. Past
// ten logins a field above the table narrows it to those whose machine,
// agent, account or plan holds what is typed (`new-talk.ts`).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AgentFacts } from "../world/agents";
import { accountOf, windowsShort } from "./agent-homes";

export type PickerHold = "mine" | "shared" | "others" | "assist";

/** One machine as the picker shows it. */
export interface PickerMachine {
  hold: PickerHold;
  written: string;
  online: boolean | null;
  /** What follows its name: this computer, whose it is, offline. */
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
  noAgents: string;
  noMachine: string;
  token: string;
  onCredits: string;
  /** A usage window past the reset it was read before. */
  reset: string;
}

const HOLDS: PickerHold[] = ["mine", "shared", "others", "assist"];

/** Past this many logins the table can be narrowed by what is typed. */
export const FILTER_PAST = 10;
const programName = (agent: string) => (agent === "codex" ? "Codex" : agent === "claude" ? "Claude" : agent);
const markOf = (agent: string) => (agent === "claude" ? "claude" : "codex");

export function agentPicker(machines: PickerMachine[], words: PickerWords): string {
  const body = HOLDS.map((hold) => {
    const held = machines.filter((machine) => machine.hold === hold);
    if (!held.length) return "";
    return `<div class="agent-hold"><h3 class="agent-group">${escape(words.holds[hold])}</h3>${held.map((machine) => machineRows(machine, words)).join("")}</div>`;
  }).join("");
  return `<div class="agent-picker" role="radiogroup" aria-label="${escape(words.agent)}">${body || `<p class="agent-none">${escape(words.noMachine)}</p>`}</div>`;
}

function machineRows(machine: PickerMachine, words: PickerWords): string {
  const state = machine.online === false ? " offline" : machine.online === true ? " online" : "";
  const head = `<div class="agent-machine${state}"><span class="agent-machine-dot" aria-hidden="true"></span><span class="agent-machine-name">${escape(machine.written)}</span>${machine.notes.map((note) => `<span class="agent-machine-note">${escape(note)}</span>`).join("")}</div>`;
  const host = (inner: string) => `<div class="agent-host" data-find="${escape(machine.written.toLowerCase())}">${inner}</div>`;
  if (machine.online === false) return host(head);
  const rows = machine.rows.length ? machine.rows.map((row) => rowHtml(row, words)).join("") : `<p class="agent-none quiet">${escape(words.noAgents)}</p>`;
  return host(head + rows);
}

function rowHtml(row: PickerRow, words: PickerWords): string {
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
  const said = [name, row.who, plan, windows.join(" · "), note].filter(Boolean).join(" · ");
  const kind = row.note ? "" : row.caution ? " caution" : credits ? " credits" : "";
  const find = [name, row.who, plan].filter(Boolean).join(" ").toLowerCase();
  return `<button type="button" class="agent-row${row.chosen ? " chosen" : ""}" role="radio" aria-checked="${row.chosen}" aria-label="${escape(said)}" title="${escape(said)}" data-choice="pick" data-value="${escape(row.key)}" data-find="${escape(find)}"${row.startable ? "" : " disabled"}>
    <span class="agent-row-mark">${glyph(markOf(row.agent), 16)}</span>
    <span class="agent-row-agent">${escape(name)}</span>
    <span class="agent-row-who">${escape(row.who ?? "")}</span>
    <span class="agent-row-plan">${plan ? `<span class="tag">${escape(plan)}</span>` : ""}</span>
    <span class="agent-row-limits">${limits}</span>
    <span class="agent-row-note${kind}">${note ? escape(note) : ""}</span>
  </button>`;
}

/** Narrows the table drawn in `root` to what holds `typed`: a machine
 *  whose name holds it keeps all its logins; otherwise the logins whose
 *  agent, account or plan holds it, under their machines; a group with
 *  none left goes. Nothing is narrowed for nothing typed. */
export function narrowPicker(root: HTMLElement, typed: string): void {
  const query = typed.trim().toLowerCase();
  for (const hold of root.querySelectorAll<HTMLElement>(".agent-hold")) {
    let any = false;
    for (const host of hold.querySelectorAll<HTMLElement>(".agent-host")) {
      const named = !query || (host.dataset.find ?? "").includes(query);
      let rows = false;
      for (const row of host.querySelectorAll<HTMLElement>(".agent-row")) {
        const shown = named || (row.dataset.find ?? "").includes(query);
        row.hidden = !shown;
        rows ||= shown;
      }
      for (const none of host.querySelectorAll<HTMLElement>(".agent-none")) none.hidden = !named;
      host.hidden = !named && !rows;
      any ||= !host.hidden;
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
