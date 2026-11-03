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
import { agentMark, glyph } from "../conversation/glyphs";
import { LongText, type LongTextWords } from "../conversation/long-text";
import { escape } from "../conversation/prose";
import { AnchorLayout } from "../longview";
import { connect as connectFilm } from "../film/connect";
import { Film } from "../film/film";
import type { Change, ChangePreview, ChangeStep, LoginChange, OutputLimit, Step } from "../world/logins";
import { previewDiff, shortPath } from "./change-diff";
import { discloseCards, drawDisclosed, moved, toggleDisclosed } from "./disclose";
import { enDisclose, zhDisclose, type DiscloseWords } from "./disclose-words";
import { Sheet } from "./confirm";
import { detailOf, enProblems, known, zhProblems } from "./problem";
import { stepMark as mark } from "../pen/step-mark";
import { accountLayer } from "./layer";

export interface AgentChangeWords {
  applyTitle(who: string, machine: string): string;
  removeTitle(who: string, machine: string): string;
  putIn: string;
  putInWhat: string;
  putInTags: string[];
  turnOff: string;
  turnOffWhat(agent: string): string;
  turnOffTags(agent: string): string[];
  leave: string;
  leaveWhat: string;
  leaveTags: string[];
  restore: string;
  restoreWhat: string;
  kept: string;
  keepsWhat: string;
  openSessions(agent: string, into: boolean): string;
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
  /** The window told as cards, from a node that works the change out so. */
  disclose: DiscloseWords;
}

export const zhAgentChange: AgentChangeWords = {
  applyTitle: (who, machine) => `接入 ${machine} 上的 ${who}`,
  removeTitle: (who, machine) => `移除 ${machine} 上的 ${who}`,
  putIn: "换上",
  putInWhat: "Fastctx 的 run：执行命令、查看文件与图片、修改文件、访问其他机器",
  putInTags: ["run"],
  turnOff: "关掉",
  turnOffWhat: (agent) => (agent === "claude" ? "它自带的 Bash（Windows 上连同 PowerShell）" : "它自带的 shell"),
  turnOffTags: (agent) => (agent === "claude" ? ["Bash", "PowerShell"] : ["shell"]),
  leave: "不动",
  leaveWhat: "其余一切：读写文件、查看图片、待办、子代理、插件、联网搜索",
  leaveTags: ["写文件", "待办", "子代理", "联网搜索"],
  restore: "还原",
  restoreWhat: "按记录还原接入时的改动",
  kept: "保留",
  keepsWhat: "你此后的改动",
  openSessions: (agent, into) =>
    into
      ? agent === "claude"
        ? "已开启的 Claude Code 会话即刻停用自带的 Bash，重新打开后方可使用 run"
        : "已开启的 Codex 会话需重新打开，方可使用 run"
      : "已开启的会话需重新打开，方可恢复自带的工具",
  permission: "Fastctx 发起的 Claude 对话此后不再逐项请求批准命令与文件改动；你自行开启的会话仍逐项请求",
  programs: "程序",
  programsWhat: "使用此登录的程序一并获得 run",
  details: "细节",
  hideDetails: "收起细节",
  reading: "正在计算改动",
  created: "新建",
  changed: "修改",
  removed: "删除",
  withheld: "可能为你自己的值，不予显示",
  lines: { more: (n) => `+ ${n.toLocaleString("zh-CN")} 行`, less: "− 收起", full: "整屏查看", english: false, unchanged: (n) => `${n} 行未改动` },
  raise: (limit) => `把输出上限从 ${limit.current.toLocaleString("zh-CN")} 调到 ${limit.requested.toLocaleString("zh-CN")}（${limit.location}，所有工具共用）`,
  takeBack: "你改动过的 Fastctx 配置恢复为当前版本的写法，文件其余部分不变",
  steps: { program: "安置程序", command: (command) => `运行 ${command}`, files: "修改配置与说明", check: (scope) => `核对 ${scope}` },
  cancel: "取消",
  apply: "接入",
  remove: "移除",
  close: "关闭",
  done: "已接入",
  removedDone: "已移除",
  failed: (reason) => known(reason, zhProblems) ?? "未能完成",
  previewFailed: "无法计算改动",
  elsewhere: (machine) => `由 ${machine} 自行修改`,
  disclose: zhDisclose,
};

export const enAgentChange: AgentChangeWords = {
  applyTitle: (who, machine) => `Connect ${who} on ${machine}`,
  removeTitle: (who, machine) => `Remove ${who} on ${machine}`,
  putIn: "Puts in",
  putInWhat: "Fastctx's run: commands, files and images, edits, other machines",
  putInTags: ["run"],
  turnOff: "Turns off",
  turnOffWhat: (agent) => (agent === "claude" ? "Its own Bash (and PowerShell on Windows)" : "Its own shell"),
  turnOffTags: (agent) => (agent === "claude" ? ["Bash", "PowerShell"] : ["shell"]),
  leave: "Leaves",
  leaveWhat: "Everything else: reading and writing files, images, to-dos, subagents, plugins, web search",
  leaveTags: ["Write tools", "To-dos", "Subagents", "Web search"],
  restore: "Puts back",
  restoreWhat: "What connecting changed, from its record",
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
  apply: "Connect",
  remove: "Remove",
  close: "Close",
  done: "Connected",
  removedDone: "Removed",
  failed: (reason) => known(reason, enProblems) ?? "Not done",
  previewFailed: "Couldn't work out the change",
  elsewhere: (machine) => `${machine} makes the change itself`,
  disclose: enDisclose,
};

export interface AgentChangeTarget {
  agent: string;
  /** How the window names the agent; Codex's and Claude Code's own names
   *  where absent. */
  title?: string;
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
  /** The workbench in it opened (`true`) or closed with the change. */
  workbench?: boolean;
}

export interface AgentChangeSeams {
  preview(target: AgentChangeTarget): Promise<ChangePreview>;
  /** `expected` is the fingerprint of the change shown: the node refuses
   *  it when the files moved on since. */
  change(target: AgentChangeTarget, raise: boolean, expected?: string): Promise<LoginChange>;
  /** The change as the login's node records it while it runs. */
  progress(target: AgentChangeTarget): Promise<LoginChange | null>;
  /** The change ended (done or not): the page reads the record again. */
  ended(): void;
  /** The workbench was opened or closed in the login. */
  benched?(open: boolean): void;
}

interface Asking {
  at: "asking";
  preview: ChangePreview | null;
  problem: string | null;
  open: boolean;
  raise: boolean;
  files: Set<number>;
  /** The files moved on since the change was shown, and it was worked out
   *  again. */
  moved: boolean;
}

/** `told`: the change as it was shown, when it was told as cards; they
 *  stay as the record of what is done. */
type Running = { at: "running"; planned: Step[]; steps: ChangeStep[]; told: ChangePreview | null };
type Ended = { at: "ended"; planned: Step[]; change: LoginChange | null; problem: string | null; told: ChangePreview | null };
type State = Asking | Running | Ended;

const asking = (moved = false): Asking => ({ at: "asking", preview: null, problem: null, open: false, raise: false, files: new Set(), moved });

const agentName = (target: AgentChangeTarget) => target.title ?? (target.agent === "codex" ? "Codex" : "Claude Code");

/** How long between reads of a running change's steps: the node records
 *  each step as it is done, and a step usually takes a fraction of a
 *  second, so the ticks follow within that. */
const FOLLOW_MS = 400;

export class AgentChangeSheet {
  private readonly sheet = new Sheet("agent-change-sheet");
  private target: AgentChangeTarget | null = null;
  private state: State = asking();
  /** Told as cards: the cards' files unfolded, and whether what connecting
   *  is shows. */
  private unfolded = new Set<string>();
  private about = false;
  /** The film of what connecting is, while it shows. */
  private film: Film | null = null;

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
      } else if (act === "dc-file") {
        toggleDisclosed(event.target as HTMLElement, this.unfolded);
        this.drawDisclosure();
      } else if (act === "dc-what" && this.state.at === "asking") {
        this.about = !this.about;
        this.showAbout();
      } else if (act === "go") void this.go();
      else if (act === "cancel" || act === "close") this.sheet.close();
    });
    this.sheet.body.addEventListener("change", (event) => {
      const box = event.target as HTMLInputElement;
      if (box.dataset.raise !== undefined && this.state.at === "asking") this.state.raise = box.checked;
    });
    this.sheet.dialog.addEventListener("close", () => this.stopFilm());
  }

  open(target: AgentChangeTarget): void {
    this.stopFilm();
    this.target = target;
    this.unfolded = new Set();
    this.about = false;
    this.ask(target, false);
    this.sheet.show();
  }

  /** Works the change out and shows it; `moved` says the files moved on
   *  since it was last shown. */
  private ask(target: AgentChangeTarget, moved: boolean): void {
    this.state = asking(moved);
    this.draw();
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
    const preview = this.state.preview;
    const told = this.carded() ? preview : null;
    if (this.carded() && !told) return;
    // Told as cards, a limit the user set is raised on its card, and the
    // one button agrees to it with the rest.
    const raise = this.state.raise || !!told?.output_limit;
    const planned = plannedSteps(target, preview);
    this.state = { at: "running", planned, steps: [], told };
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
      const change = await this.seams.change(target, raise, told?.fingerprint);
      this.state = { at: "ended", planned, change, problem: change.end?.end === "failed" ? change.end.reason : null, told };
      // Taking a login out closes the workbench in it as well.
      const bench = target.workbench ?? (target.into ? undefined : false);
      if (change.end?.end === "done" && bench !== undefined) this.seams.benched?.(bench);
    } catch (error) {
      this.state = { at: "ended", planned, change: null, problem: detailOf(error), told };
    } finally {
      following = false;
      this.sheet.busy = false;
      this.seams.ended();
    }
    // The files moved on since the change was shown: nothing was done, and
    // it is worked out again for the user to look at.
    if (told && this.state.problem && moved(this.state.problem)) {
      this.ask(target, true);
      return;
    }
    this.draw();
  }

  private draw(): void {
    const target = this.target;
    if (!target) return;
    const { words } = this;
    const state = this.state;
    const cards = this.carded();
    const said = words.disclose;
    const title = cards
      ? target.workbench !== undefined
        ? (target.workbench ? said.benchOpen : said.benchClose)(agentName(target))
        : (target.into ? said.connect : said.remove)(agentName(target))
      : target.into
        ? words.applyTitle(target.who, target.machine)
        : words.removeTitle(target.who, target.machine);
    const what = `<button class="icon-button dc-what" type="button" data-change="dc-what" aria-pressed="${this.about}" aria-label="${escape(said.what)}" title="${escape(said.what)}">${glyph("help", 18)}</button>`;
    const head =
      cards && state.at === "asking"
        ? `<header class="sheet-head"><h2 class="sheet-title">${escape(title)}</h2><div class="sheet-head-end">${target.workbench === undefined ? what : ""}<button class="icon-button sheet-close" type="button" aria-label="${escape(words.close)}">${glyph("close", 18)}</button></div></header>`
        : this.sheet.head(title, words.close);
    this.sheet.body.innerHTML = `${head}<div class="agent-change">${this.body(target)}</div>`;
    this.drawDetails();
    this.drawDisclosure();
    this.placeFilm();
  }

  /** Whether the window is told as cards. A node that works the change out
   *  as cards says so with its fingerprint; until it is worked out, it is. */
  private carded(): boolean {
    const state = this.state;
    if (state.at !== "asking") return state.told !== null;
    return !state.preview || state.preview.fingerprint !== undefined;
  }

  /** The cards' unfolded files, drawn once each. */
  private drawDisclosure(): void {
    const state = this.state;
    const preview = state.at === "asking" ? state.preview : state.told;
    if (!preview || !this.carded()) return;
    drawDisclosed(this.sheet.body, preview, this.unfolded, this.words.disclose, this.words.lines);
  }

  /** What connecting is, under the window's head while asked for: its
   *  film goes in once drawn (`placeFilm`). */
  private aboutPanel(opening: boolean): string {
    return `<div class="dc-about${opening ? " opening" : ""}"><div class="dc-about-inner"></div></div>`;
  }

  /** The film of what connecting is, in its panel while that shows: drawn
   *  again, the window takes it as it was, playing where it had got to. */
  private placeFilm(): void {
    const target = this.target;
    const inner = this.sheet.body.querySelector<HTMLElement>(".dc-about-inner");
    if (!inner || !target) return this.stopFilm();
    const said = this.words.disclose;
    this.film ??= new Film((shape) => connectFilm(said.film(agentName(target)), agentMark(target.agent), shape), said.about(agentName(target)));
    if (this.film.element.parentElement !== inner) inner.append(this.film.element);
  }

  private stopFilm(): void {
    this.film?.destroy();
    this.film = null;
  }

  /** What connecting is shown or put away, the cards left as they are. */
  private showAbout(): void {
    const target = this.target;
    if (this.state.at !== "asking" || !target) return;
    this.sheet.body.querySelector(".dc-what")?.setAttribute("aria-pressed", String(this.about));
    const shown = this.sheet.body.querySelector(".dc-about");
    const scroller = this.sheet.body.querySelector<HTMLElement>(".dc-scroll");
    if (!this.about) shown?.remove();
    else if (!shown && scroller) {
      // It opens at the top of what scrolls, which comes to it.
      scroller.insertAdjacentHTML("afterbegin", this.aboutPanel(true));
      scroller.scrollTop = 0;
    }
    this.placeFilm();
  }

  /** The cards of a change, a limit the user set on its card; and the
   *  sentences under them, which stay as the cards scroll: the World's
   *  lines they changed put back, sessions already open. */
  private cards(target: AgentChangeTarget, preview: ChangePreview): { list: string; notes: string } {
    const said = this.words.disclose;
    const limit = target.into ? preview.output_limit : undefined;
    const also: Partial<Record<Change, string>> = {};
    if (limit) also.limits = said.raised(limit.current, limit.requested);
    const notes = target.workbench !== undefined ? [] : [...(target.takeBack ? [said.takeBack] : []), said.sessions(target.agent, agentName(target), target.into)];
    return {
      list: discloseCards(preview, said, this.unfolded, also, target.workbench),
      notes: notes.length ? `<ul class="dc-notes">${notes.map((line) => `<li>${escape(line)}</li>`).join("")}</ul>` : "",
    };
  }

  /** The window as cards: which login, what connecting is when asked, a
   *  card each kind of change, the way back and the one button. */
  private disclosure(target: AgentChangeTarget, state: Asking): string {
    const { words } = this;
    const said = words.disclose;
    const { preview, problem } = state;
    const told = preview ? this.cards(target, preview) : null;
    const middle = told
      ? told.list
      : problem
        ? `<p class="removal-said removal-wrong" title="${escape(problem)}">${escape(words.previewFailed)}</p>`
        : `<p class="removal-said">${escape(words.reading)}</p>`;
    return `<div class="dc">
      <p class="dc-where">${escape(said.where(target.machine, target.who))}</p>
      <div class="dc-scroll">
        ${this.about ? this.aboutPanel(false) : ""}
        ${state.moved ? `<p class="dc-moved">${escape(said.moved)}</p>` : ""}
        ${middle}
      </div>
      ${told?.notes ?? ""}
      <div class="dc-foot">
        <span class="dc-undo">${preview ? escape(target.workbench !== undefined ? (target.workbench ? said.benchUndo : said.benchUndone) : target.into ? said.undo : said.undone) : ""}</span>
        <button class="text-button" type="button" data-change="cancel">${escape(words.cancel)}</button>
        <button class="${target.into ? "sheet-go" : "removal-button"}" type="button" data-change="go"${preview ? "" : " disabled"}>${escape(target.workbench !== undefined ? (target.workbench ? said.benchGo : said.benchStop) : target.into ? said.apply : said.take)}</button>
      </div>
    </div>`;
  }

  /** Pressed, the cards stay as the record of what is done; its steps are
   *  ticked off along the foot as the node takes them, and it ends on the
   *  stamp, or on what went wrong. */
  private afterwards(target: AgentChangeTarget, state: Running | Ended, told: ChangePreview): string {
    const { words } = this;
    const ended = state.at === "ended" ? state : null;
    const change = ended?.change ?? null;
    const done = change?.end?.end === "done";
    const kept = change?.end?.end === "done" ? (change.end.kept ?? []) : [];
    const steps = state.at === "running" ? state.steps : (change?.steps ?? []);
    const { list, notes } = this.cards(target, told);
    return `<div class="dc">
      <p class="dc-where">${escape(words.disclose.where(target.machine, target.who))}</p>
      <div class="dc-scroll">${list}</div>
      ${notes}
      ${kept.length ? `<ul class="agent-change-kept">${kept.map((one) => `<li title="${escape(one.reason)}"><span>${escape(words.kept)}</span><code>${escape(one.location)}</code></li>`).join("")}</ul>` : ""}
      ${ended?.problem ? `<p class="removal-said removal-wrong" title="${escape(ended.problem)}">${escape(words.failed(ended.problem))}</p>` : ""}
      <div class="dc-foot">
        <ol class="removal-list agent-change-steps dc-steps">${this.steps(state.planned, steps)}</ol>
        ${done ? `<p class="agent-change-stamp dc-stamp">${stampDrawing()}<span>${escape(target.workbench !== undefined ? words.disclose.benchStamp(target.workbench) : target.into ? words.done : words.removedDone)}</span></p>` : ""}
        ${ended ? `<button class="sheet-go" type="button" data-change="close">${escape(words.close)}</button>` : ""}
      </div>
    </div>`;
  }

  private body(target: AgentChangeTarget): string {
    const { words } = this;
    const state = this.state;
    if (state.at === "asking" && this.carded()) return this.disclosure(target, state);
    if (state.at !== "asking" && state.told) return this.afterwards(target, state, state.told);
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
      // Only the workbench opens Claude conversations of its own.
      ...(target.into && target.agent === "claude" && accountLayer.workbench.peek() ? [words.permission] : []),
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
    const agent = this.target ? agentName(this.target) : "";
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
 *  agent's tools when it connects; the commands and the files when it is
 *  taken out. */
function plannedSteps(target: AgentChangeTarget, preview: ChangePreview | null): Step[] {
  const commands: Step[] = (preview?.commands ?? []).map((command) => ({ step: "command", command }));
  return target.into ? [{ step: "program" }, ...commands, { step: "files" }, { step: "check", scope: "" }] : [...commands, { step: "files" }];
}

/** The agent's own hands, struck through, and the World's run beside them:
 *  drawn one way to connect, the other to remove. The interface's marks are
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
