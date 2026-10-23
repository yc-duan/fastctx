// Hooking an agent's login up to the World, or removing it (`world-gui.md`,
// 账号、机器、共享与协助): a real change to the agent, so a card of its own.
// A drawing of its own hands giving way to `run`; what is put in, turned off
// and left alone, each a short label and a few tags, the sentence behind
// them on hover; the programs that use the login, as tags; what happens to
// sessions already open; for Claude, the permission that comes with it.
// Under 「细节」, folded at first, the real preview, worked out with nothing
// written: a file to a line with its lines added and removed, opened as a
// change reads in a conversation; it scrolls in its own place, the card's
// head and foot staying put. Pressed, each step is ticked off in pencil as
// the login's node does it, and the card ends on the stamp. Removing is the
// same card, saying what is put back and what the user changed since that
// stays.

import { Sketch } from "../pen/sketch";
import { counts, parse } from "../conversation/diff";
import { diffPaint, memoryDiff } from "../conversation/diff-lines";
import { glyph } from "../conversation/glyphs";
import { LongText, type LongTextWords } from "../conversation/long-text";
import { escape } from "../conversation/prose";
import { AnchorLayout } from "../longview";
import type { ChangePreview, ChangeStep, LoginChange, OutputLimit, Step } from "../world/logins";
import { previewDiff, shortPath } from "./change-diff";
import { Sheet } from "./confirm";
import { detailOf, enProblems, known, zhProblems } from "./problem";
import { stepMark as mark } from "../pen/step-mark";

export interface AgentChangeWords {
  applyTitle(who: string, machine: string): string;
  removeTitle(who: string, machine: string): string;
  putIn: string;
  putInWhat: string;
  putInTags: string[];
  turnOff: string;
  turnOffWhat(agent: "codex" | "claude"): string;
  turnOffTags(agent: "codex" | "claude"): string[];
  leave: string;
  leaveWhat: string;
  leaveTags: string[];
  restore: string;
  restoreWhat: string;
  kept: string;
  keepsWhat: string;
  openSessions(agent: "codex" | "claude", into: boolean): string;
  permission: string;
  programs: string;
  programsWhat: string;
  details: string;
  hideDetails: string;
  reading: string;
  created: string;
  changed: string;
  removed: string;
  withheld: string;
  raise(limit: OutputLimit): string;
  /** Said when what the user changed of the World's lines goes back. */
  takeBack: string;
  /** A change's lines, as a conversation says them. */
  lines: LongTextWords & { unchanged(n: number): string };
  steps: { program: string; command(command: string): string; files: string; check(scope: string): string };
  cancel: string;
  apply: string;
  remove: string;
  close: string;
  done: string;
  removedDone: string;
  /** A change that did not happen, in the interface's words; the
   *  kernel's own reason goes in the hover. */
  failed(reason: string): string;
  previewFailed: string;
  elsewhere(machine: string): string;
}

export const zhAgentChange: AgentChangeWords = {
  applyTitle: (who, machine) => `接入 ${machine} 上的 ${who}`,
  removeTitle: (who, machine) => `移除 ${machine} 上的 ${who}`,
  putIn: "换上",
  putInWhat: "Fastctx 的 run：跑命令、看文件与图片、改文件、去别的机器",
  putInTags: ["run"],
  turnOff: "关掉",
  turnOffWhat: (agent) => (agent === "claude" ? "它自带的 Bash、Grep、Glob（Windows 上连同 PowerShell）" : "它自带的 shell 与看图"),
  turnOffTags: (agent) => (agent === "claude" ? ["Bash", "Grep", "Glob", "PowerShell"] : ["shell", "看图"]),
  leave: "不动",
  leaveWhat: "其余一切：写文件的工具、子代理、插件、联网搜索、你的设置与说明",
  leaveTags: ["写文件", "子代理", "插件", "联网搜索", "你的设置"],
  restore: "还原",
  restoreWhat: "接入时改的，照记录还原",
  kept: "留着",
  keepsWhat: "你后来改的",
  openSessions: (agent, into) =>
    into
      ? agent === "claude"
        ? "已经开着的 Claude Code 会话马上用不了自带的 Bash，重新打开才有 run"
        : "已经开着的 Codex 会话要重新打开才有 run"
      : "已经开着的会话要重新打开才回到自带的工具",
  permission: "Fastctx 开的 Claude 对话从此不再逐条问你批准命令和改文件；你自己开的会话照旧逐条问",
  programs: "程序",
  programsWhat: "用这个登录的程序一起有了 run",
  details: "细节",
  hideDetails: "收起细节",
  reading: "正在算要改什么",
  created: "新建",
  changed: "修改",
  removed: "删除",
  withheld: "可能是你自己的值，不显示",
  lines: { more: (n) => `+ ${n.toLocaleString("zh-CN")} 行`, less: "− 收起", full: "整屏查看", english: false, unchanged: (n) => `${n} 行未改动` },
  raise: (limit) => `把输出上限从 ${limit.current.toLocaleString("zh-CN")} 调到 ${limit.requested.toLocaleString("zh-CN")}（${limit.location}，所有工具共用）`,
  takeBack: "你改过的 Fastctx 那几处换回它现在写的样子，文件里别的地方不动",
  steps: { program: "放好程序", command: (command) => `运行 ${command}`, files: "改配置与说明", check: (scope) => `核对 ${scope}` },
  cancel: "取消",
  apply: "接入",
  remove: "移除",
  close: "关闭",
  done: "已接入",
  removedDone: "已移除",
  failed: (reason) => known(reason, zhProblems) ?? "没办成",
  previewFailed: "算不出要改什么",
  elsewhere: (machine) => `由 ${machine} 自己去改`,
};

export const enAgentChange: AgentChangeWords = {
  applyTitle: (who, machine) => `Hook up ${who} on ${machine}`,
  removeTitle: (who, machine) => `Remove ${who} on ${machine}`,
  putIn: "Puts in",
  putInWhat: "Fastctx's run: commands, files and images, edits, other machines",
  putInTags: ["run"],
  turnOff: "Turns off",
  turnOffWhat: (agent) => (agent === "claude" ? "Its own Bash, Grep and Glob (and PowerShell on Windows)" : "Its own shell and image viewer"),
  turnOffTags: (agent) => (agent === "claude" ? ["Bash", "Grep", "Glob", "PowerShell"] : ["shell", "image viewer"]),
  leave: "Leaves",
  leaveWhat: "Everything else: its write tools, subagents, plugins, web search, your settings and guidance",
  leaveTags: ["Write tools", "Subagents", "Plugins", "Web search", "Your settings"],
  restore: "Puts back",
  restoreWhat: "What hooking up changed, from its record",
  kept: "Keeps",
  keepsWhat: "What you changed since",
  openSessions: (agent, into) =>
    into
      ? agent === "claude"
        ? "Claude Code sessions already open lose their own Bash at once and get run once reopened"
        : "Codex sessions already open get run once reopened"
      : "Sessions already open get their own tools back once reopened",
  permission: "Claude conversations Fastctx opens stop asking you to approve each command and edit; sessions you open ask as before",
  programs: "Programs",
  programsWhat: "Programs using this login get run too",
  details: "Details",
  hideDetails: "Hide details",
  reading: "Working out what changes",
  created: "New",
  changed: "Changed",
  removed: "Removed",
  withheld: "May be your own value; not shown",
  lines: { more: (n) => `+ ${n.toLocaleString("en")} lines`, less: "− show less", full: "Full screen", english: true, unchanged: (n) => (n === 1 ? "1 unchanged line" : `${n} unchanged lines`) },
  raise: (limit) => `Raise the output limit from ${limit.current.toLocaleString("en")} to ${limit.requested.toLocaleString("en")} (${limit.location}, shared by every tool)`,
  takeBack: "What you changed of Fastctx's lines goes back to how this version writes them; the rest of each file stays",
  steps: { program: "Put the program in place", command: (command) => `Run ${command}`, files: "Change settings and guidance", check: (scope) => `Check ${scope}` },
  cancel: "Cancel",
  apply: "Hook up",
  remove: "Remove",
  close: "Close",
  done: "Hooked up",
  removedDone: "Removed",
  failed: (reason) => known(reason, enProblems) ?? "Not done",
  previewFailed: "Couldn't work out the change",
  elsewhere: (machine) => `${machine} makes the change itself`,
};

export interface AgentChangeTarget {
  agent: "codex" | "claude";
  machine: string;
  dir: string;
  /** How the slip names the login. */
  who: string;
  /** The programs that use this login: a name each, what it is and where
   *  on hover, and whether it comes inside another program. */
  programs: { name: string; detail: string; within: boolean }[];
  /** Whether the login is on this computer. */
  here: boolean;
  into: boolean;
  /** What the user changed of the World's tools in it is rewritten. */
  takeBack?: boolean;
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
  | { at: "asking"; preview: ChangePreview | null; problem: string | null; open: boolean; raise: boolean; files: Set<number> }
  | { at: "running"; planned: Step[]; steps: ChangeStep[] }
  | { at: "ended"; planned: Step[]; change: LoginChange | null; problem: string | null };

/** How long between reads of a running change's steps: the node records
 *  each step as it is done, and a step usually takes a fraction of a
 *  second, so the ticks follow within that. */
const FOLLOW_MS = 400;

export class AgentChangeSheet {
  private readonly sheet = new Sheet("agent-change-sheet");
  private target: AgentChangeTarget | null = null;
  private state: State = { at: "asking", preview: null, problem: null, open: false, raise: false, files: new Set() };

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
      } else if (act === "file" && this.state.at === "asking") {
        const index = Number((event.target as HTMLElement).closest<HTMLElement>("[data-file]")?.dataset.file);
        if (this.state.files.has(index)) this.state.files.delete(index);
        else this.state.files.add(index);
        this.drawDetails();
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
    this.state = { at: "asking", preview: null, problem: null, open: false, raise: false, files: new Set() };
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
    const planned = plannedSteps(target, this.state.preview);
    this.state = { at: "running", planned, steps: [] };
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
      this.state = { at: "ended", planned, change, problem: change.end?.end === "failed" ? change.end.reason : null };
    } catch (error) {
      this.state = { at: "ended", planned, change: null, problem: detailOf(error) };
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
    this.drawDetails();
  }

  private body(target: AgentChangeTarget): string {
    const { words } = this;
    const state = this.state;
    const tags = (names: string[]) => names.map((name) => `<span class="agent-change-tag">${escape(name)}</span>`).join("");
    const row = (name: string, what: string, inside: string) => `<div title="${escape(what)}"><dt>${escape(name)}</dt><dd>${inside}</dd></div>`;
    const rows = target.into
      ? [
          row(words.putIn, words.putInWhat, tags(words.putInTags)),
          row(words.turnOff, words.turnOffWhat(target.agent), tags(words.turnOffTags(target.agent))),
          row(words.leave, words.leaveWhat, tags(words.leaveTags)),
          ...(target.programs.length
            ? [
                row(
                  words.programs,
                  words.programsWhat,
                  target.programs
                    .map((program) => `<span class="agent-change-tag" title="${escape(program.detail)}">${glyph(program.within ? "desktop" : "terminal", 14)}${escape(program.name)}</span>`)
                    .join(""),
                ),
              ]
            : []),
        ]
      : [row(words.restore, words.restoreWhat, `<span class="agent-change-said">${escape(words.restoreWhat)}</span>`), row(words.kept, words.keepsWhat, `<span class="agent-change-said">${escape(words.keepsWhat)}</span>`)];
    const lines = [
      ...(target.takeBack ? [words.takeBack] : []),
      words.openSessions(target.agent, target.into),
      ...(target.into && target.agent === "claude" ? [words.permission] : []),
      ...(target.here ? [] : [words.elsewhere(target.machine)]),
    ];
    const head = `${handsDrawing(target.into)}
      <dl class="agent-change-rows">${rows.join("")}</dl>
      <ul class="agent-change-lines">${lines.map((line) => `<li>${escape(line)}</li>`).join("")}</ul>`;
    if (state.at === "asking") {
      const preview = state.preview;
      const limit = target.into ? preview?.output_limit : undefined;
      const details = state.open ? `<div class="agent-change-details">${this.details(preview, state.problem)}</div>` : "";
      return `${head}
        ${limit ? `<label class="agent-change-raise"><input type="checkbox" data-raise${state.raise ? " checked" : ""}><span>${escape(words.raise(limit))}</span></label>` : ""}
        <button class="text-button removal-toggle agent-change-toggle" type="button" data-change="details" aria-expanded="${state.open}">${glyph("chevron", 14)}<span>${escape(state.open ? words.hideDetails : words.details)}</span></button>
        ${details}
        <div class="removal-go">
          <button class="text-button" type="button" data-change="cancel">${escape(words.cancel)}</button>
          <button class="${target.into ? "sheet-go" : "removal-button"}" type="button" data-change="go"${!preview && !state.problem ? " disabled" : ""}>${escape(target.into ? words.apply : words.remove)}</button>
        </div>`;
    }
    if (state.at === "running") return `${head}<ol class="removal-list agent-change-steps">${this.steps(state.planned, state.steps)}</ol>`;
    const change = state.change;
    const done = change?.end?.end === "done";
    const kept = change?.end?.end === "done" ? (change.end.kept ?? []) : [];
    return `${head}
      ${change ? `<ol class="removal-list agent-change-steps">${this.steps(state.planned, change.steps)}</ol>` : ""}
      ${done ? `<p class="agent-change-stamp">${stampDrawing()}<span>${escape(target.into ? words.done : words.removedDone)}</span></p>` : ""}
      ${kept.length ? `<ul class="agent-change-kept">${kept.map((one) => `<li title="${escape(one.reason)}"><span>${escape(words.kept)}</span><code>${escape(one.location)}</code></li>`).join("")}</ul>` : ""}
      ${state.problem ? `<p class="removal-said removal-wrong" title="${escape(state.problem)}">${escape(words.failed(state.problem))}</p>` : ""}
      <div class="removal-go"><button class="sheet-go" type="button" data-change="close">${escape(words.close)}</button></div>`;
  }

  /** The files that would change, a line each: its short path and the
   *  lines added and removed; an opened one shows the change. */
  private details(preview: ChangePreview | null, problem: string | null): string {
    const { words } = this;
    if (problem) return `<p class="removal-said removal-wrong" title="${escape(problem)}">${escape(words.previewFailed)}</p>`;
    if (!preview) return `<p class="removal-said">${escape(words.reading)}</p>`;
    const open = this.state.at === "asking" ? this.state.files : new Set<number>();
    const files = preview.files
      .map((file, index) => {
        const { added, removed } = counts(previewDiff(file, words.withheld).join("\n"));
        const mark = file.how === "created" ? "plus" : file.how === "removed" ? "minus" : "edit";
        const said = file.how === "created" ? words.created : file.how === "removed" ? words.removed : words.changed;
        return `<li class="agent-change-file${open.has(index) ? " open" : ""}" data-file="${index}">
          <button class="agent-change-file-line" type="button" data-change="file" aria-expanded="${open.has(index)}" title="${escape(said)}\n${escape(file.path)}">
            <span class="agent-change-file-mark">${glyph(mark, 14)}</span><span class="agent-change-path">${escape(shortPath(file.path))}</span>
            <span class="row-stats"><span class="add">+${added}</span><span class="del">−${removed}</span></span><span class="agent-change-chevron">${glyph("chevron", 14)}</span>
          </button>
          <div class="agent-change-diff"></div>
        </li>`;
      })
      .join("");
    const commands = (preview.commands ?? []).map((command) => `<li class="agent-change-command"><span class="agent-change-file-mark">${glyph("terminal", 14)}</span><code>${escape(command)}</code></li>`).join("");
    return `<ul class="agent-change-files">${files}${commands}</ul>`;
  }

  /** The change of each opened file, as a conversation shows a change; it
   *  scrolls with the details, in their own place on the card. */
  private drawDetails(): void {
    const state = this.state;
    if (state.at !== "asking" || !state.preview) return;
    const scroller = this.sheet.body.querySelector<HTMLElement>(".agent-change-details");
    const list = scroller?.querySelector<HTMLElement>(".agent-change-files");
    if (!scroller || !list) return;
    const layout = new AnchorLayout(scroller);
    const { words } = this;
    state.preview.files.forEach((file, index) => {
      const item = list.querySelector<HTMLElement>(`[data-file="${index}"]`);
      if (!item) return;
      const opened = state.files.has(index);
      item.classList.toggle("open", opened);
      item.querySelector(".agent-change-file-line")?.setAttribute("aria-expanded", String(opened));
      const holder = item.querySelector<HTMLElement>(".agent-change-diff")!;
      if (!opened) {
        holder.replaceChildren();
        return;
      }
      if (holder.childElementCount) return;
      const rows = parse(previewDiff(file, words.withheld).join("\n"));
      const lines = memoryDiff(rows);
      holder.append(new LongText({ ...lines, paint: diffPaint(file.path, words.lines.unchanged, false), kind: "diff" }, shortPath(file.path), layout, words.lines).element);
    });
  }

  /** Every step the change takes, listed before the first begins, each
   *  with what the node has said of it so far. A step said takes the place
   *  of the first one listed of its kind; one never listed goes last. */
  private rows(planned: Step[], said: ChangeStep[]): string[] {
    const { words } = this;
    const left = [...said];
    const take = (kind: Step["step"]) => {
      const at = left.findIndex((one) => one.step.step === kind);
      return at < 0 ? null : left.splice(at, 1)[0]!;
    };
    const ones = [...planned.map((step) => take(step.step) ?? { step, state: null }), ...left];
    const agent = this.target?.agent === "codex" ? "Codex" : "Claude Code";
    return ones.map((one) => {
      const { step } = one;
      const label =
        step.step === "program" ? words.steps.program : step.step === "command" ? words.steps.command(step.command) : step.step === "files" ? words.steps.files : words.steps.check(agent);
      const state = one.state?.state ?? "ahead";
      const result = state === "done" || state === "failed" ? state : null;
      const title = one.state?.state === "failed" ? one.state.reason : step.step === "check" ? step.scope : "";
      return `<li data-state="${state}"${result ? ` data-result="${result}"` : ""}${title ? ` title="${escape(title)}"` : ""}><span class="removal-mark" aria-hidden="true">${mark(result)}</span><span class="removal-what">${escape(label)}</span></li>`;
    });
  }

  private steps(planned: Step[], said: ChangeStep[]): string {
    return this.rows(planned, said).join("");
  }

  /** The steps as the node said them last: only a row that changed is drawn
   *  again, so a tick drawn once is not drawn in anew at every reading. */
  private drawSteps(): void {
    if (this.state.at !== "running") return;
    const list = this.sheet.body.querySelector<HTMLElement>(".agent-change-steps");
    if (!list) return;
    this.rows(this.state.planned, this.state.steps).forEach((html, index) => {
      const shown = list.children[index] as HTMLElement | undefined;
      if (shown?.dataset.html === html) return;
      const row = document.createElement("template");
      row.innerHTML = html;
      const item = row.content.firstElementChild as HTMLElement;
      item.dataset.html = html;
      if (shown) shown.replaceWith(item);
      else list.append(item);
    });
  }
}

/** The steps a change will take, in the order the node takes them: the
 *  program, Claude Code's own commands, the files, and a check of the
 *  agent's tools when it hooks up; the commands and the files when it is
 *  taken out. */
function plannedSteps(target: AgentChangeTarget, preview: ChangePreview | null): Step[] {
  const commands: Step[] = (preview?.commands ?? []).map((command) => ({ step: "command", command }));
  return target.into ? [{ step: "program" }, ...commands, { step: "files" }, { step: "check", scope: "" }] : [...commands, { step: "files" }];
}

/** The agent's own hands, struck through, and the World's run beside them:
 *  drawn one way to hook up, the other to remove. The interface's marks are
 *  drawn as the interface draws them; the strike, the way across and the
 *  ring round the World's are the pen's. */
function handsDrawing(into: boolean): string {
  const sheet = new Sketch(into ? "change-into" : "change-out").ring([125, 37], 22, 1.6, "clay");
  if (into)
    sheet.stroke(
      [
        [8, 70],
        [22, 52],
        [36, 30],
        [48, 8],
      ],
      2.2,
      "clay",
      0.1,
    );
  const [from, to] = into ? [58, 102] : [102, 58];
  const head = into ? -7 : 7;
  sheet
    .stroke(
      [
        [from, 38],
        [(from + to) / 2, 32.5],
        [to, 37],
      ],
      1.6,
      "graphite",
      0.08,
    )
    .stroke(
      [
        [to + head, 31],
        [to, 37],
        [to + head, 42],
      ],
      1.6,
      "graphite",
      0.2,
    );
  const own = `<g class="ach-own"><g transform="translate(14 14)">${glyph("terminal", 20)}</g><g transform="translate(14 40)">${glyph("image", 20)}</g></g>`;
  const run = `<g class="ach-run" transform="translate(112 24)">${glyph("terminal", 26)}</g>`;
  return sheet.svg(150, 76, "agent-change-drawing").replace("</svg>", `${own}${run}</svg>`);
}

/** Done: a clay ring with a tick in it. */
function stampDrawing(): string {
  return new Sketch("change-stamp")
    .ring([12, 12], 9, 1.5, "clay")
    .stroke(
      [
        [7.5, 12.3],
        [10.6, 15.2],
        [16.6, 8.8],
      ],
      1.8,
      "clay",
      0.2,
    )
    .svg(24, 24, "agent-change-stampmark");
}
