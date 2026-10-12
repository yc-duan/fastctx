// Hooking an agent's login up to the World, or removing it (`world-gui.md`,
// 账号、机器、共享与借出): a real change to the agent, so a card of its own.
// A drawing of its own hands giving way to `run`; three short lines of what
// is put in, what is turned off and what is left alone; what happens to
// sessions already open; for Claude, the permission that comes with it;
// under 「细节」 the real preview, worked out with nothing written. Pressed,
// each step is ticked off in pencil as the login's node does it, and the
// card ends on the stamp. Removing is the same card, saying what is put
// back and what the user changed since that stays.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { ChangePreview, ChangeStep, LoginChange, OutputLimit } from "../world/logins";
import { Sheet } from "./confirm";
import { detailOf } from "./problem";
import { mark } from "./removal";

export interface AgentChangeWords {
  applyTitle(who: string, machine: string): string;
  removeTitle(who: string, machine: string): string;
  putIn: string;
  putInWhat: string;
  turnOff: string;
  turnOffWhat(agent: "codex" | "claude"): string;
  leave: string;
  leaveWhat: string;
  restore: string;
  restoreWhat: string;
  keepsWhat: string;
  openSessions(agent: "codex" | "claude", into: boolean): string;
  permission: string;
  programs(names: string): string;
  details: string;
  hideDetails: string;
  reading: string;
  created: string;
  changed: string;
  removed: string;
  withheld: string;
  kept: string;
  raise(limit: OutputLimit): string;
  steps: { program: string; command(command: string): string; files: string; check(scope: string): string };
  cancel: string;
  apply: string;
  remove: string;
  close: string;
  done: string;
  removedDone: string;
  failed(reason: string): string;
  previewFailed: string;
  elsewhere(machine: string): string;
}

export const zhAgentChange: AgentChangeWords = {
  applyTitle: (who, machine) => `接入 ${who} · ${machine}`,
  removeTitle: (who, machine) => `移除 ${who} · ${machine}`,
  putIn: "换上",
  putInWhat: "Fastctx 的 run：跑命令、看文件与图片、改文件、去别的机器",
  turnOff: "关掉",
  turnOffWhat: (agent) => (agent === "claude" ? "它自带的 Bash、Grep、Glob（Windows 上连同 PowerShell）" : "它自带的 shell 与看图"),
  leave: "不动",
  leaveWhat: "其余一切：写文件的工具、子代理、插件、联网搜索、你的设置与说明",
  restore: "还原",
  restoreWhat: "接入时改过的，照记录还原成接入之前的样子",
  keepsWhat: "接入之后你自己改过的，留着",
  openSessions: (agent, into) =>
    into
      ? agent === "claude"
        ? "已经开着的 Claude Code 会话马上用不了自带的 Bash，重新打开才有 run"
        : "已经开着的 Codex 会话要重新打开才有 run"
      : "已经开着的会话要重新打开才回到自带的工具",
  permission: "Fastctx 开的 Claude 对话从此不再逐条问你批准命令和改文件；你自己开的会话照旧逐条问",
  programs: (names) => `用这个登录的程序一起有了 run：${names}`,
  details: "细节",
  hideDetails: "收起细节",
  reading: "正在算要改什么",
  created: "新建",
  changed: "修改",
  removed: "删除",
  withheld: "可能是你自己的值，不显示",
  kept: "留着",
  raise: (limit) => `把输出上限从 ${limit.current.toLocaleString("zh-CN")} 调到 ${limit.requested.toLocaleString("zh-CN")}（${limit.location}，所有工具共用）`,
  steps: { program: "放好程序", command: (command) => `运行 ${command}`, files: "改配置与说明", check: (scope) => `核对 ${scope}` },
  cancel: "取消",
  apply: "接入",
  remove: "移除",
  close: "关闭",
  done: "已接入",
  removedDone: "已移除",
  failed: (reason) => `没办成：${reason}`,
  previewFailed: "算不出要改什么",
  elsewhere: (machine) => `由 ${machine} 自己去改`,
};

export const enAgentChange: AgentChangeWords = {
  applyTitle: (who, machine) => `Hook up ${who} · ${machine}`,
  removeTitle: (who, machine) => `Remove ${who} · ${machine}`,
  putIn: "Puts in",
  putInWhat: "Fastctx's run: commands, files and images, edits, other machines",
  turnOff: "Turns off",
  turnOffWhat: (agent) => (agent === "claude" ? "Its own Bash, Grep and Glob (and PowerShell on Windows)" : "Its own shell and image viewer"),
  leave: "Leaves",
  leaveWhat: "Everything else: its write tools, subagents, plugins, web search, your settings and guidance",
  restore: "Puts back",
  restoreWhat: "What hooking up changed, as it was before",
  keepsWhat: "What you changed since stays",
  openSessions: (agent, into) =>
    into
      ? agent === "claude"
        ? "Claude Code sessions already open lose their own Bash at once and get run once reopened"
        : "Codex sessions already open get run once reopened"
      : "Sessions already open get their own tools back once reopened",
  permission: "Claude conversations Fastctx opens stop asking you to approve each command and edit; sessions you open ask as before",
  programs: (names) => `Programs using this login get run too: ${names}`,
  details: "Details",
  hideDetails: "Hide details",
  reading: "Working out what changes",
  created: "New",
  changed: "Changed",
  removed: "Removed",
  withheld: "May be your own value; not shown",
  kept: "Kept",
  raise: (limit) => `Raise the output limit from ${limit.current.toLocaleString("en")} to ${limit.requested.toLocaleString("en")} (${limit.location}, shared by every tool)`,
  steps: { program: "Put the program in place", command: (command) => `Run ${command}`, files: "Change settings and guidance", check: (scope) => `Check ${scope}` },
  cancel: "Cancel",
  apply: "Hook up",
  remove: "Remove",
  close: "Close",
  done: "Hooked up",
  removedDone: "Removed",
  failed: (reason) => `Not done: ${reason}`,
  previewFailed: "Couldn't work out the change",
  elsewhere: (machine) => `${machine} makes the change itself`,
};

export interface AgentChangeTarget {
  agent: "codex" | "claude";
  machine: string;
  dir: string;
  /** How the slip names the login. */
  who: string;
  /** The programs that use this login, as names. */
  programs: string[];
  /** Whether the login is on this computer. */
  here: boolean;
  into: boolean;
}

export interface AgentChangeSeams {
  preview(target: AgentChangeTarget): Promise<ChangePreview>;
  change(target: AgentChangeTarget, raise: boolean): Promise<LoginChange>;
  /** The change as the login's node records it while it runs. */
  progress(target: AgentChangeTarget): Promise<LoginChange | null>;
  /** The change ended (done or not): the page reads the record again. */
  ended(): void;
}

type State =
  | { at: "asking"; preview: ChangePreview | null; problem: string | null; open: boolean; raise: boolean }
  | { at: "running"; steps: ChangeStep[] }
  | { at: "ended"; change: LoginChange | null; problem: string | null };

/** How long between reads of a running change's steps: the node records
 *  each step as it is done, and a step usually takes a fraction of a
 *  second, so the ticks follow within that. */
const FOLLOW_MS = 400;

export class AgentChangeSheet {
  private readonly sheet = new Sheet("agent-change-sheet");
  private target: AgentChangeTarget | null = null;
  private state: State = { at: "asking", preview: null, problem: null, open: false, raise: false };

  constructor(
    private readonly words: AgentChangeWords,
    private readonly seams: AgentChangeSeams,
  ) {
    this.sheet.body.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLElement>("[data-change]")?.dataset.change;
      if (!act) return;
      event.preventDefault();
      if (act === "details" && this.state.at === "asking") {
        this.state.open = !this.state.open;
        this.draw();
      } else if (act === "go") void this.go();
      else if (act === "cancel" || act === "close") this.sheet.close();
    });
    this.sheet.body.addEventListener("change", (event) => {
      const box = event.target as HTMLInputElement;
      if (box.dataset.raise !== undefined && this.state.at === "asking") this.state.raise = box.checked;
    });
  }

  open(target: AgentChangeTarget): void {
    this.target = target;
    this.state = { at: "asking", preview: null, problem: null, open: false, raise: false };
    this.draw();
    this.sheet.show();
    this.seams.preview(target).then(
      (preview) => {
        if (this.target !== target || this.state.at !== "asking") return;
        this.state.preview = preview;
        this.draw();
      },
      (error: unknown) => {
        if (this.target !== target || this.state.at !== "asking") return;
        this.state.problem = detailOf(error);
        this.draw();
      },
    );
  }

  private async go(): Promise<void> {
    const target = this.target;
    if (!target || this.state.at !== "asking") return;
    const raise = this.state.raise;
    this.state = { at: "running", steps: [] };
    this.sheet.busy = true;
    this.draw();
    let following = true;
    const follow = async () => {
      while (following) {
        const change = await this.seams.progress(target).catch(() => null);
        if (!following) return;
        if (change && this.state.at === "running") {
          this.state.steps = change.steps;
          this.drawSteps();
        }
        await new Promise((resolve) => setTimeout(resolve, FOLLOW_MS));
      }
    };
    void follow();
    try {
      const change = await this.seams.change(target, raise);
      this.state = { at: "ended", change, problem: change.end?.end === "failed" ? change.end.reason : null };
    } catch (error) {
      this.state = { at: "ended", change: null, problem: detailOf(error) };
    } finally {
      following = false;
      this.sheet.busy = false;
      this.seams.ended();
    }
    this.draw();
  }

  private draw(): void {
    const target = this.target;
    if (!target) return;
    const { words } = this;
    const title = target.into ? words.applyTitle(target.who, target.machine) : words.removeTitle(target.who, target.machine);
    this.sheet.body.innerHTML = `${this.sheet.head(title, words.close)}<div class="agent-change">${this.body(target)}</div>`;
  }

  private body(target: AgentChangeTarget): string {
    const { words } = this;
    const state = this.state;
    const rows = target.into
      ? [
          [words.putIn, words.putInWhat],
          [words.turnOff, words.turnOffWhat(target.agent)],
          [words.leave, words.leaveWhat],
        ]
      : [
          [words.restore, words.restoreWhat],
          [words.leave, words.keepsWhat],
        ];
    const lines = [
      words.openSessions(target.agent, target.into),
      ...(target.into && target.agent === "claude" ? [words.permission] : []),
      ...(target.into && target.programs.length ? [words.programs(target.programs.join("、"))] : []),
      ...(target.here ? [] : [words.elsewhere(target.machine)]),
    ];
    const head = `${handsDrawing(target.into)}
      <dl class="agent-change-rows">${rows.map(([name, what]) => `<div><dt>${escape(name!)}</dt><dd>${escape(what!)}</dd></div>`).join("")}</dl>
      <ul class="agent-change-lines">${lines.map((line) => `<li>${escape(line)}</li>`).join("")}</ul>`;
    if (state.at === "asking") {
      const preview = state.preview;
      const limit = target.into ? preview?.output_limit : undefined;
      const details = state.open ? this.details(preview, state.problem) : "";
      return `${head}
        ${limit ? `<label class="agent-change-raise"><input type="checkbox" data-raise${state.raise ? " checked" : ""}><span>${escape(words.raise(limit))}</span></label>` : ""}
        <button class="text-button removal-toggle" type="button" data-change="details" aria-expanded="${state.open}">${escape(state.open ? words.hideDetails : words.details)}</button>
        ${details}
        <div class="removal-go">
          <button class="text-button" type="button" data-change="cancel">${escape(words.cancel)}</button>
          <button class="${target.into ? "sheet-go" : "removal-button"}" type="button" data-change="go"${!preview && !state.problem ? " disabled" : ""}>${escape(target.into ? words.apply : words.remove)}</button>
        </div>`;
    }
    if (state.at === "running") return `${head}<ol class="removal-list agent-change-steps">${this.steps(state.steps)}</ol>`;
    const change = state.change;
    const done = change?.end?.end === "done";
    const kept = change?.end?.end === "done" ? (change.end.kept ?? []) : [];
    return `${head}
      ${change ? `<ol class="removal-list agent-change-steps">${this.steps(change.steps)}</ol>` : ""}
      ${done ? `<p class="agent-change-stamp">${stampDrawing()}<span>${escape(target.into ? words.done : words.removedDone)}</span></p>` : ""}
      ${kept.length ? `<ul class="agent-change-kept">${kept.map((one) => `<li title="${escape(one.reason)}"><span>${escape(words.kept)}</span><code>${escape(one.location)}</code></li>`).join("")}</ul>` : ""}
      ${state.problem ? `<p class="removal-said removal-wrong" title="${escape(state.problem)}">${escape(words.failed(state.problem))}</p>` : ""}
      <div class="removal-go"><button class="sheet-go" type="button" data-change="close">${escape(words.close)}</button></div>`;
  }

  private details(preview: ChangePreview | null, problem: string | null): string {
    const { words } = this;
    if (problem) return `<p class="removal-said removal-wrong" title="${escape(problem)}">${escape(words.previewFailed)}</p>`;
    if (!preview) return `<p class="removal-said">${escape(words.reading)}</p>`;
    const how = (value: "created" | "changed" | "removed") => (value === "created" ? words.created : value === "removed" ? words.removed : words.changed);
    const files = preview.files
      .map(
        (file) => `<li><div class="agent-change-file"><span class="tag">${escape(how(file.how))}</span><code>${escape(file.path)}</code></div>
          <ul class="agent-change-items">${file.items
            .map((item) => {
              const values = item.withheld ? `<span class="quiet" title="${escape(words.withheld)}">…</span>` : `${item.before !== undefined ? `<del>${escape(item.before)}</del>` : ""}${item.after !== undefined ? `<ins>${escape(item.after)}</ins>` : ""}`;
              return `<li><span class="agent-change-item">${escape(item.item)}</span>${values}</li>`;
            })
            .join("")}</ul></li>`,
      )
      .join("");
    const commands = (preview.commands ?? []).map((command) => `<li><code>${escape(command)}</code></li>`).join("");
    return `<ul class="agent-change-files">${files}${commands}</ul>`;
  }

  private steps(steps: ChangeStep[]): string {
    const { words } = this;
    return steps
      .map((one) => {
        const label =
          one.step.step === "program"
            ? words.steps.program
            : one.step.step === "command"
              ? words.steps.command(one.step.command)
              : one.step.step === "files"
                ? words.steps.files
                : words.steps.check(one.step.scope);
        const result = one.state.state === "done" ? "done" : one.state.state === "failed" ? "failed" : null;
        const why = one.state.state === "failed" ? one.state.reason : "";
        return `<li${result ? ` data-result="${result}"` : ""}${why ? ` title="${escape(why)}"` : ""}><span class="removal-mark" aria-hidden="true">${mark(result)}</span><span class="removal-what">${escape(label)}</span></li>`;
      })
      .join("");
  }

  private drawSteps(): void {
    if (this.state.at !== "running") return;
    const list = this.sheet.body.querySelector<HTMLElement>(".agent-change-steps");
    if (list) list.innerHTML = this.steps(this.state.steps);
  }
}

/** The agent's own hands, struck through, and the World's run beside them:
 *  drawn one way to hook up, the other to remove. */
function handsDrawing(into: boolean): string {
  const own = `<g class="ach-own">${`<g transform="translate(14 14)">${glyph("terminal", 20)}</g><g transform="translate(14 40)">${glyph("image", 20)}</g>`}</g>`;
  const run = `<g class="ach-run" transform="translate(112 24)">${glyph("terminal", 26)}</g>`;
  const strike = `<path class="ach-strike" d="M8 70 C 24 50, 36 30, 48 8"/>`;
  const arrow = `<path class="ach-arrow" d="${into ? "M58 38 C 74 32, 88 32, 102 37 M95 31 L102 37 L94 42" : "M102 38 C 86 32, 72 32, 58 37 M65 31 L58 37 L66 42"}"/>`;
  return `<svg class="agent-change-drawing" viewBox="0 0 150 76" aria-hidden="true"><g filter="url(#pen-marker-lite)">${own}${into ? strike : ""}${arrow}<circle class="ach-ring" cx="125" cy="37" r="22"/>${run}</g></svg>`;
}

function stampDrawing(): string {
  return `<svg class="agent-change-stampmark" viewBox="0 0 24 24" aria-hidden="true" filter="url(#pen-marker)"><circle cx="12" cy="12" r="9"/><path d="M7.5 12.3 L10.6 15.2 L16.6 8.8"/></svg>`;
}
