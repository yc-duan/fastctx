// What a member of the World may use and do (`world-network.md`, 管理权与
// 范围), as the interface says it and lets the user set it: a line that
// says it in plain words, and the fields that set each part of it. A part
// is everything, or only the things chosen; choosing one thing narrows it,
// choosing "all" widens it again.

import { escape } from "../conversation/prose";
import { scopeActions, type Scope, type ScopeAction, type ScopeSet, type Standing } from "../world/world";
import type { ScopeWords } from "./words";

/** Everything a member may be given, inviting aside. */
export function everything(): Scope {
  return { machines: "all", agents: "all", projects: "all", actions: "all", invite: false };
}

const agents = ["codex", "claude"] as const;

function agentName(agent: string): string {
  return agent === "codex" ? "Codex" : agent === "claude" ? "Claude Code" : agent;
}

/** A scope said in one line: where and which agents, which projects, what;
 *  what is everything is said once. */
export function describe(scope: Scope, words: ScopeWords): string {
  const join = <T>(items: T[], name: (item: T) => string) => items.map(name).join(words.and);
  const { machines, agents, projects, actions } = scope;
  if (machines === "all" && agents === "all" && projects === "all" && actions === "all") {
    return scope.invite ? `${words.everything}${words.sep}${words.canInvite}` : words.everything;
  }
  const where =
    machines !== "all" && !machines.length
      ? words.noMachines
      : agents !== "all" && !agents.length
        ? words.noAgents
        : words.where(machines === "all" ? null : join(machines, (m) => m), agents === "all" ? null : join(agents, agentName));
  const parts = [
    where,
    projects === "all" ? words.allProjects : projects.length ? words.projectsOnly(join(projects, (p) => p)) : words.noProjects,
    actions === "all" ? words.allActions : actions.length ? join(actions, (a) => words.action[a]) : words.noActions,
  ];
  if (scope.invite) parts.push(words.canInvite);
  return parts.join(words.sep);
}

/** Where a member stands, in a line: what it may use; management rights
 *  are said by a tag beside it. */
export function standingLine(standing: Standing, words: ScopeWords): string {
  if (standing.manage) return words.manages;
  return describe(standing.scope, words);
}

export interface ScopeChoices {
  /** The machines a scope can name: every machine of the World, and the
   *  one being invited when it has a name. */
  machines: string[];
  /** The projects, by their short names. */
  projects: string[];
  /** Whether the user may hand on the right to invite: only a member with
   *  management rights gives it. */
  invite: boolean;
  /** Whether management rights are given here too. */
  manage: { held: boolean } | null;
}

type Part = "machines" | "agents" | "projects" | "actions";

/** The fields that set a scope, inside a sheet's form. */
export class ScopeEditor {
  private scope: Scope;
  private manage: boolean;

  constructor(
    private readonly words: ScopeWords,
    private readonly choices: ScopeChoices,
    scope: Scope,
  ) {
    this.scope = structuredClone(scope);
    this.manage = choices.manage?.held ?? false;
  }

  /** The fields as markup; `hook` wires them once they are in the page. */
  html(): string {
    const { words, choices } = this;
    const part = <T extends string>(name: Part, label: string, all: string, items: readonly T[], title: (item: T) => string) => {
      const set = this.scope[name] as ScopeSet<T>;
      const options = items
        .map((item) => {
          const on = set !== "all" && set.includes(item);
          return `<button type="button" class="choice${on ? " chosen" : ""}" aria-pressed="${on}" data-part="${name}" data-item="${escape(item)}">${escape(title(item))}</button>`;
        })
        .join("");
      return `<div class="field scope-part" data-part-field="${name}"><span class="field-label">${escape(label)}</span>
        <div class="choice-options"><button type="button" class="choice${set === "all" ? " chosen" : ""}" aria-pressed="${set === "all"}" data-part="${name}" data-all="1">${escape(all)}</button>${options}</div></div>`;
    };
    const manage = choices.manage
      ? `<label class="scope-switch"><input type="checkbox" name="manage"${this.manage ? " checked" : ""}><span><strong>${escape(words.manage)}</strong><span class="field-hint">${escape(words.manageHint)}</span></span></label>`
      : "";
    const invite = choices.invite
      ? `<label class="scope-switch"><input type="checkbox" name="invite"${this.scope.invite ? " checked" : ""}><span><strong>${escape(words.invite)}</strong><span class="field-hint">${escape(words.inviteHint)}</span></span></label>`
      : "";
    return `<div class="scope-editor${this.manage ? " managing" : ""}">
      ${manage}
      <div class="scope-parts">
        ${part("machines", words.machines, words.allMachines, choices.machines, (m) => m)}
        ${part("agents", words.agents, words.allAgents, agents, agentName)}
        ${choices.projects.length ? part("projects", words.projects, words.allProjects, choices.projects, (p) => p) : ""}
        ${part("actions", words.actions, words.allActions, scopeActions, (a: ScopeAction) => words.action[a])}
        ${invite}
      </div>
      <p class="scope-said field-hint" aria-live="polite">${escape(this.said())}</p>
    </div>`;
  }

  /** What the fields now say, in one line. */
  private said(): string {
    return this.manage ? this.words.manages : describe(this.scope, this.words);
  }

  /** Wires the fields inside `root`, which holds `html()`. */
  hook(root: HTMLElement): void {
    root.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-part]");
      if (!button) return;
      const name = button.dataset.part as Part;
      const current = this.scope[name] as ScopeSet<string>;
      if (button.dataset.all) {
        this.scope[name] = "all" as never;
      } else {
        const item = button.dataset.item!;
        const chosen = current === "all" ? [] : [...current];
        const at = chosen.indexOf(item);
        if (at >= 0) chosen.splice(at, 1);
        else chosen.push(item);
        this.scope[name] = chosen as never;
      }
      this.redraw(root, name);
    });
    root.addEventListener("change", (event) => {
      const input = event.target as HTMLInputElement;
      if (input.name === "invite") this.scope.invite = input.checked;
      if (input.name === "manage") {
        this.manage = input.checked;
        root.querySelector(".scope-editor")?.classList.toggle("managing", this.manage);
      }
      this.say(root);
    });
  }

  private redraw(root: HTMLElement, name: Part): void {
    const set = this.scope[name] as ScopeSet<string>;
    for (const button of root.querySelectorAll<HTMLButtonElement>(`[data-part="${name}"]`)) {
      const on = button.dataset.all ? set === "all" : set !== "all" && set.includes(button.dataset.item!);
      button.classList.toggle("chosen", on);
      button.setAttribute("aria-pressed", String(on));
    }
    this.say(root);
  }

  private say(root: HTMLElement): void {
    const said = root.querySelector<HTMLElement>(".scope-said");
    if (said) said.textContent = this.said();
  }

  /** The scope the fields set. */
  value(): Scope {
    return structuredClone(this.scope);
  }

  /** Whether management rights are given. */
  managing(): boolean {
    return this.manage;
  }
}
