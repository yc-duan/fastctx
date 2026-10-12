import type { HealthKind } from "./world/events";
import type { FileLinkWords } from "./conversation/file-links";
import type { PictureWords } from "./conversation/pictures";
import type { QueueWords } from "./conversation/queue";
// Every word the interface says on its own account, in the user's words
// (`world-gui.md`, 用词): plain, without role-play; what agents and tools
// produce is never translated.

import type { FindWords } from "./conversation/find";
import type { OutputWords } from "./conversation/run-output";
import { enQuestions, zhQuestions, type QuestionWords } from "./conversation/question-card";

export type Tense = "running" | "done" | "failed";

/** What a tool did, before it is put into words. */
export type Act =
  | "read"
  | "edit"
  | "create"
  | "delete"
  | "move"
  | "write"
  | "command"
  | "search"
  | "image"
  | "ask"
  | "message"
  | "fetch"
  | "websearch"
  | "tasks";

export interface Tally {
  read: number;
  edited: number;
  ran: number;
  searched: number;
  asked: number;
  other: number;
  failed: number;
  thinking: number;
}

export interface Words {
  english: boolean;
  queue: QueueWords;
  /** A card of questions asked without waiting. */
  questions: QuestionWords;
  act(act: Act, tense: Tense): string;
  failedTool(name: string): string;
  tally(tally: Tally): string;
  thought: string;
  thinking: string;
  thoughtWithheld: string;
  /** A thought's row where the host gave none of its words. */
  thoughtHidden: string;
  plan: string;
  copyMessage: string;
  copyCode: string;
  rewind: string;
  fork: string;
  /** Said with the source already escaped as a link. */
  forkedFrom(source: string): string;
  find: FindWords;
  you(machine: string): string;
  world: string;
  /** An open page, as the speaker of what it said. */
  aPage(name: string): string;
  compacted: string;
  turn(how: string, reason: string | null): string | null;
  wentOnAs(job: string): string;
  /** On a tool's row: it failed, where nothing else says so. */
  rowFailed: string;
  exited(code: number): string;
  cut: string;
  unknown: string;
  declined: string;
  showMore: string;
  showLess: string;
  copy: string;
  copied: string;
  sendPlaceholder(agent: string): string;
  send: string;
  stop: string;
  uncertain(why: string): string;
  sending: string;
  context(percent: number): string;
  working: string;
  /** The turn's line while a conversation's first words are on their way
   *  and nothing has come back yet. */
  starting: string;
  /** Under the first words, when the conversation could not begin; the
   *  reason follows as it was given. */
  startFailed: string;
  /** The turn's line while the agent writes its answer. */
  answering: string;
  idle: string;
  /** The user removed it from the World: kept to read, not to speak to. */
  removed: string;
  removedNote: string;
  waitingForYou: string;
  on(machine: string): string;
  /** Under a conversation's title: the machine its commands start on,
   *  where it is not the one it lives on. */
  worksOn(machine: string): string;
  /** Under the title of a conversation with an agent lent to the user. */
  lentBy(owner: string): string;
  /** Between the names in a short list. */
  listSeparator: string;
  /** The button under the first lines of something long: "+ 28 行". */
  moreLines(n: number): string;
  unchangedLines(n: number): string;
  /** Long text opened full screen. */
  fullScreen: string;
  /** What a command printed and what a write changed, opened under their
   *  rows: what was left out of what the model got, the whole output on
   *  its machine beside it, and what is said when either is gone. */
  output: OutputWords & {
    modelCopy: string;
    whole: string;
    wholeGone: string;
    diffGone: string;
    reading: string;
    readFailed: string;
  };
  /** How long something took: "1 分 12 秒". */
  took(ms: number): string;
  /** What a turn took, under its info mark: the model and effort the host
   *  reported, how long, and the tokens. */
  turnInfo: {
    model: string;
    effort: string;
    requested(effort: string): string;
    took: string;
    tokens: string;
    spent(input: number, cached: number, output: number, reasoning: number): string;
  };
  /** How many steps a stretch of work has taken. */
  steps(n: number): string;
  /** Under a turn's last message: copy all it said, ask the same again. */
  copyAnswer: string;
  regenerate: string;
  /** A job's end, told to the session, as one line. */
  jobDone(code: number | null): string;
  jobStoppedByYou: string;
  jobStopped: string;
  jobInterrupted: string;
  jobCouldNotRun: string;
  /** A colleague handed back its answer: "GPT · laptop 交回 · 14 分钟". */
  answered(label: string, took: string | null): string;
  answerFailed(label: string): string;
  /** The watch on the session ended: the node ended it, or the connection
   *  to it broke. */
  watchEnded(why: string | null): string;
  /** The live stream broke and is waiting to be read again. */
  watchInterrupted(why: string | null): string;
  /** Words that could not be sent. */
  notSent(why: string): string;
  /** What the session's host said of its own health, in a plain line; the
   *  host's words go on hover. */
  health: Record<HealthKind, string>;
  /** Links an agent writes to files. */
  files: FileLinkWords;
  /** The conversation's pictures, small and over the whole window. */
  pictures: PictureWords;
  /** The way into a colleague's own conversation. */
  openColleague: string;
  /** Under Claude Code's agent tool: its subagent's kind, steps taken, the one it takes now. */
  subagent(kind: string, steps: number, doing: string | null): string;
  /** What it started that still runs. */
  stillRunning: string;
  /** Before a job is stopped by the user, and when it could not be. */
  stopJobAsk: string;
  notStopped: string;
  cancel: string;
  /** A job's output opened under its row. */
  jobOutput(command: string): string;
  jobOutputLoading: string;
  jobOutputFailed(why: string): string;
  jobOver: string;
  /** A job running on a machine for so long. */
  jobRunning(job: string, machine: string, lasted: string): string;
  minutes(n: number): string;
  hours(hours: number, minutes: number): string;
  /** Words in the World but in no turn yet: waiting for the session's next
   *  call of a World tool, gone as a letter, or handed over without knowing
   *  whether they went in. */
  landed(how: string, machine: string, why: string | null): string;
  /** A job the resident started has ended, on that machine. */
  jobEnded(job: string, machine: string): string;
  /** The user moved it to another project through the World. */
  movedTo(town: string): string;
  movedOut: string;
  /** The project's brief it was handed with the move, folded. */
  briefHanded: string;
  deskFree(who: string): string;
  deskTaken(who: string): string;
  deskOutside: string;
  /** How a job or a colleague's turn ended, from the World's own wording
   *  (`exited 0`, `turn completed, 14m`); a wording not known is kept. */
  ended(how: string): string;
}

const zhActs: Record<Act, [string, string, string]> = {
  read: ["读取", "读取中", "读取失败"],
  edit: ["修改", "修改中", "修改失败"],
  create: ["新建", "新建中", "新建失败"],
  delete: ["删除", "删除中", "删除失败"],
  move: ["移动", "移动中", "移动失败"],
  write: ["写入", "写入中", "写入失败"],
  command: ["运行", "运行中", "命令失败"],
  search: ["搜索", "搜索中", "搜索失败"],
  image: ["看图", "查看中", "查看失败"],
  ask: ["调用", "调用中", "调用失败"],
  message: ["发送", "发送中", "发送失败"],
  fetch: ["获取", "获取中", "获取失败"],
  websearch: ["网页搜索", "搜索中", "搜索失败"],
  tasks: ["待办", "更新中", "更新失败"],
};

const enActs: Record<Act, [string, string, string]> = {
  read: ["Read", "Reading", "Read Failed"],
  edit: ["Edited", "Editing", "Edit Failed"],
  create: ["Created", "Creating", "Create Failed"],
  delete: ["Deleted", "Deleting", "Delete Failed"],
  move: ["Moved", "Moving", "Move Failed"],
  write: ["Wrote", "Writing", "Write Failed"],
  command: ["Ran", "Running command", "Command Failed"],
  search: ["Searched", "Searching", "Search Failed"],
  image: ["Viewed", "Viewing image", "Image Failed"],
  ask: ["Called", "Calling a model", "Call Failed"],
  message: ["Sent", "Writing to a model", "Message Failed"],
  fetch: ["Fetched", "Fetching", "Fetch Failed"],
  websearch: ["Searched the web", "Searching the web", "Web search Failed"],
  tasks: ["Tasks", "Updating tasks", "Tasks Failed"],
};

const index = (tense: Tense) => (tense === "done" ? 0 : tense === "running" ? 1 : 2);

export const zh: Words = {
  english: false,
  questions: zhQuestions,
  queue: {
    waiting: (count) => `排队 ${count} 条`,
    held: "已暂停 · 本轮停止",
    notTaken: "已暂停 · 上一条没送出",
    unsure: "不知是否送到",
    sendNow: "立即发送",
    sendNowTitle: "不等本轮结束，立即发送",
    edit: "编辑",
    save: "保存",
    cancel: "取消",
    remove: "移除",
    reorder: "拖动调整顺序（或 Alt+↑↓）",
    files: (count) => `${count} 个文件`,
    empty: "队列为空",
    queue: "加入队列",
    paused: "已暂停",
    done: "完成",
    sending: "发送中",
    editingElsewhere: "别处正在改",
  },
  act: (act, tense) => zhActs[act][index(tense)],
  failedTool: (name) => `${name} 失败`,
  tally: (t) => {
    const parts: string[] = [];
    if (t.read) parts.push(`读取 ${t.read} 个文件`);
    if (t.edited) parts.push(`修改 ${t.edited} 个文件`);
    if (t.ran) parts.push(`运行 ${t.ran} 条命令`);
    if (t.searched) parts.push(`搜索 ${t.searched} 次`);
    if (t.asked) parts.push(`调用模型 ${t.asked} 次`);
    if (t.other) parts.push(`调用 ${t.other} 个工具`);
    let heading = parts.join(" · ");
    if (!heading) heading = t.thinking ? "思考" : "活动";
    return t.failed ? `${heading} · ${t.failed} 个失败` : heading;
  },
  thought: "思考",
  thinking: "正在思考",
  thoughtWithheld: "思考内容未公开",
  thoughtHidden: "内容未公开",
  plan: "计划",
  copyMessage: "复制",
  copyCode: "复制代码",
  rewind: "改了重发",
  fork: "从这里分叉",
  forkedFrom: (source) => `分叉自 ${source}`,
  find: {
    find: "查找",
    placeholder: "在对话中查找…",
    count: (current, total) => `${current} / ${total}`,
    none: "无结果",
    previous: "上一个",
    next: "下一个",
    close: "关闭查找",
  },
  you: (machine) => `你 · ${machine}`,
  world: "世界",
  aPage: (name) => `页面「${name}」`,
  compacted: "上下文已压缩",
  turn: (how, reason) => {
    if (how === "completed") return null;
    const said = how === "failed" ? "本轮失败" : how === "interrupted" ? "本轮已停止" : `本轮结束：${how}`;
    return reason ? `${said}：${reason}` : said;
  },
  wentOnAs: (job) => `转入后台 ${job}`,
  rowFailed: "失败",
  exited: (code) => `退出 ${code}`,
  cut: "本轮结束时仍在运行",
  unknown: "结果未知",
  declined: "已拒绝",
  showMore: "展开全部",
  showLess: "− 收起",
  copy: "复制",
  copied: "已复制",
  sendPlaceholder: (agent) => `发消息给 ${agent}…`,
  send: "发送",
  stop: "停止",
  uncertain: () => "送达状态未知",
  sending: "发送中",
  context: (percent) => `上下文 ${percent}%`,
  working: "工作中",
  starting: "启动中",
  startFailed: "启动失败",
  answering: "书写回答",
  idle: "空闲",
  removed: "已移除",
  removedNote: "已移除，对话仅供查看",
  waitingForYou: "等待回复",
  on: (machine) => machine,
  worksOn: (machine) => `工作于 ${machine}`,
  lentBy: (owner) => `来自 ${owner}`,
  listSeparator: "、",
  moreLines: (n) => `+ ${n.toLocaleString("zh-CN")} 行`,
  unchangedLines: (n) => `${n} 行未改动`,
  fullScreen: "整屏查看",
  output: {
    linesLeftOut: (n) => `⋯ 省略 ${n.toLocaleString("zh-CN")} 行`,
    charsLeftOut: (n) => `⋯ 省略 ${n.toLocaleString("zh-CN")} 个字符`,
    unreadable: "此行无法显示",
    modelCopy: "模型拿到的",
    whole: "完整输出",
    wholeGone: "完整输出已不在那台机器上",
    diffGone: "改动的细节已不在那台机器上",
    reading: "读取中…",
    readFailed: "读取失败",
  },
  turnInfo: {
    model: "模型",
    effort: "推理强度",
    requested: (effort) => `${effort}（请求）`,
    took: "用时",
    tokens: "用量",
    spent: (input, cached, output, reasoning) =>
      [`输入 ${input.toLocaleString("zh-CN")}`, cached ? `缓存命中 ${cached.toLocaleString("zh-CN")}` : "", `输出 ${output.toLocaleString("zh-CN")}`, reasoning ? `其中推理 ${reasoning.toLocaleString("zh-CN")}` : ""]
        .filter(Boolean)
        .join(" · "),
  },
  took: (ms) => {
    const seconds = Math.max(0, Math.round(ms / 1000));
    if (seconds < 60) return `${seconds} 秒`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return seconds % 60 ? `${minutes} 分 ${seconds % 60} 秒` : `${minutes} 分`;
    return minutes % 60 ? `${Math.floor(minutes / 60)} 小时 ${minutes % 60} 分` : `${Math.floor(minutes / 60)} 小时`;
  },
  steps: (n) => `${n} 步`,
  copyAnswer: "复制回答",
  regenerate: "重新回答",
  jobDone: (code) => (code === null ? "后台命令已结束" : `后台命令已结束 · 退出 ${code}`),
  jobStoppedByYou: "后台命令已被你停下",
  jobStopped: "后台命令已停下",
  jobInterrupted: "后台命令中断",
  jobCouldNotRun: "后台命令没能运行",
  answered: (label, took) => (took ? `${label} 交回 · ${took}` : `${label} 交回`),
  answerFailed: (label) => `${label} 本轮失败`,
  watchEnded: () => "已停止同步",
  watchInterrupted: () => "连接中断，正在重连…",
  notSent: () => "发送失败",
  health: {
    reconnecting: "连接中断，正在重试",
    safety: "OpenAI 正在对这一轮的输出做额外的安全审查",
    rerouted: "OpenAI 把这一轮换到了另一个模型",
    tools_missing: "世界的工具没有接上",
    tool_server: "有一组工具没能启动",
    warning: "有一条提醒",
    error: "这一轮出错了",
  },
  files: {
    reveal: "在文件夹中显示",
    copyPath: "复制路径",
    copied: "已复制路径",
    on: (machine, path) => `${machine} 上的 ${path}`,
    openFailed: (why) => `打不开这个文件：${why}`,
    gone: "原件已不在",
  },
  pictures: {
    close: "关闭",
    previous: "上一张",
    next: "下一张",
    copyImage: "复制图片",
    saveAs: "另存为",
    copyPath: "复制路径",
    reveal: "在文件夹中显示",
    copied: "已复制",
    saved: "已保存",
    gone: "原件已不在",
    failed: (why) => `没能取到：${why}`,
    counter: (at, of) => `${at} / ${of}`,
  },
  openColleague: "查看",
  subagent: (kind, steps, doing) => [`子代理${kind ? ` ${kind}` : ""}`, `${steps} 步`, doing].filter(Boolean).join(" · "),
  stillRunning: "运行中的命令",
  stopJobAsk: "停下这条命令？会告诉智能体是你停的。",
  notStopped: "没能停下",
  cancel: "取消",
  jobOutput: (command) => `${command} 的输出`,
  jobOutputLoading: "正在读取…",
  jobOutputFailed: (why) => `读不到输出：${why}`,
  jobOver: "已结束",
  jobRunning: (job, machine, lasted) => `${job} · ${machine} · ${lasted}`,
  minutes: (n) => (n < 1 ? "不到 1 分钟" : `${n} 分钟`),
  hours: (hours, minutes) => (minutes ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`),
  landed: (how, machine) =>
    how === "sent"
      ? `已发往 ${machine}`
      : how === "uncertain"
        ? "不确定是否送到"
        : "等它下次用世界的工具时送到",
  jobEnded: (_job, machine) => `后台命令在 ${machine} 上结束`,
  movedTo: (town) => `已移到项目 ${town}`,
  movedOut: "已移出项目",
  briefHanded: "项目说明",
  deskFree: (who) => `${who} 这一轮结束，屏幕空出来了`,
  deskTaken: (who) => `${who} 正在操作屏幕，结束后会通知`,
  deskOutside: "屏幕被世界之外的会话占着",
  ended: (how) =>
    endedIn(how, {
      exited: (code) => `退出 ${code}`,
      signal: (signal) => `被信号 ${signal} 结束`,
      stopped: "已停止",
      finished: "已完成",
      interrupted: (reason) => `中断：${reason}`,
      couldNotRun: (error) => `启动失败：${error}`,
      turn: (how, reason) =>
        (how === "completed" ? "本轮完成" : how === "failed" ? "本轮失败" : how === "interrupted" ? "本轮中断" : `本轮 ${how}`) +
        (reason ? `：${reason}` : ""),
      inTurn: "本轮进行中",
    }),
};

export const en: Words = {
  english: true,
  questions: enQuestions,
  queue: {
    waiting: (count) => `${count} queued`,
    held: "Paused · turn stopped",
    notTaken: "Paused · last one not sent",
    unsure: "Delivery unknown",
    sendNow: "Send now",
    sendNowTitle: "Send now without waiting for this turn",
    edit: "Edit",
    save: "Save",
    cancel: "Cancel",
    remove: "Remove",
    reorder: "Drag to reorder (or Alt+↑↓)",
    files: (count) => `${count} files`,
    empty: "Queue is empty",
    queue: "Add to queue",
    paused: "Paused",
    done: "Done",
    sending: "Sending",
    editingElsewhere: "Being edited elsewhere",
  },
  act: (act, tense) => enActs[act][index(tense)],
  failedTool: (name) => `${name} Failed`,
  tally: (t) => {
    const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
    const parts: string[] = [];
    if (t.read) parts.push(`read ${plural(t.read, "file", "files")}`);
    if (t.edited) parts.push(`edited ${plural(t.edited, "file", "files")}`);
    if (t.ran) parts.push(`ran ${plural(t.ran, "command", "commands")}`);
    if (t.searched) parts.push(t.searched === 1 ? "searched once" : `searched ${t.searched} times`);
    if (t.asked) parts.push(`called ${plural(t.asked, "model", "models")}`);
    if (t.other) parts.push(plural(t.other, "tool call", "tool calls"));
    let heading = parts.join(", ");
    if (!heading) heading = t.thinking ? "Thinking" : "Activity";
    heading = heading.charAt(0).toUpperCase() + heading.slice(1);
    return t.failed ? `${heading} · ${t.failed} failed` : heading;
  },
  thought: "Thought",
  thinking: "Thinking",
  thoughtWithheld: "Thinking not shared",
  thoughtHidden: "not shared",
  plan: "Plan",
  copyMessage: "Copy",
  copyCode: "Copy code",
  rewind: "Edit and resend",
  fork: "Fork from here",
  forkedFrom: (source) => `Forked from ${source}`,
  find: {
    find: "Find",
    placeholder: "Find in conversation…",
    count: (current, total) => `${current} of ${total}`,
    none: "No matches",
    previous: "Previous",
    next: "Next",
    close: "Close find",
  },
  you: (machine) => `You · ${machine}`,
  world: "World",
  aPage: (name) => `the page “${name}”`,
  compacted: "Context compacted",
  turn: (how, reason) => {
    if (how === "completed") return null;
    const said = how === "failed" ? "Turn failed" : how === "interrupted" ? "Turn stopped" : `Turn ended: ${how}`;
    return reason ? `${said}: ${reason}` : said;
  },
  wentOnAs: (job) => `went on as ${job}`,
  rowFailed: "failed",
  exited: (code) => `exit ${code}`,
  cut: "still running when the turn ended",
  unknown: "outcome unknown",
  declined: "denied",
  showMore: "Show more",
  showLess: "− show less",
  copy: "Copy",
  copied: "Copied",
  sendPlaceholder: (agent) => `Message ${agent}…`,
  send: "Send",
  stop: "Stop",
  uncertain: () => "Delivery unknown",
  sending: "Sending",
  context: (percent) => `Context ${percent}%`,
  working: "Working",
  starting: "Starting",
  startFailed: "Couldn't start",
  answering: "Writing",
  idle: "Idle",
  removed: "Removed",
  removedNote: "Removed; the conversation is read-only",
  waitingForYou: "Waiting for your reply",
  on: (machine) => machine,
  worksOn: (machine) => `works on ${machine}`,
  lentBy: (owner) => `from ${owner}`,
  listSeparator: ", ",
  moreLines: (n) => `+ ${n.toLocaleString("en")} more line${n === 1 ? "" : "s"}`,
  unchangedLines: (n) => `${n} unchanged lines`,
  fullScreen: "Full screen",
  output: {
    linesLeftOut: (n) => `⋯ ${n.toLocaleString("en")} line${n === 1 ? "" : "s"} left out`,
    charsLeftOut: (n) => `⋯ ${n.toLocaleString("en")} character${n === 1 ? "" : "s"} left out`,
    unreadable: "This line can't be shown",
    modelCopy: "What the model got",
    whole: "Full output",
    wholeGone: "The full output is no longer on that machine",
    diffGone: "This change's details are no longer on that machine",
    reading: "Reading…",
    readFailed: "Couldn't read it",
  },
  turnInfo: {
    model: "Model",
    effort: "Effort",
    requested: (effort) => `${effort} (requested)`,
    took: "Took",
    tokens: "Usage",
    spent: (input, cached, output, reasoning) =>
      [`${input.toLocaleString("en")} in`, cached ? `${cached.toLocaleString("en")} cached` : "", `${output.toLocaleString("en")} out`, reasoning ? `${reasoning.toLocaleString("en")} reasoning` : ""]
        .filter(Boolean)
        .join(" · "),
  },
  took: (ms) => {
    const seconds = Math.max(0, Math.round(ms / 1000));
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return seconds % 60 ? `${minutes}m ${seconds % 60}s` : `${minutes}m`;
    return minutes % 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${Math.floor(minutes / 60)}h`;
  },
  steps: (n) => `${n} step${n === 1 ? "" : "s"}`,
  copyAnswer: "Copy answer",
  regenerate: "Answer again",
  jobDone: (code) => (code === null ? "Background command finished" : `Background command finished · exit ${code}`),
  jobStoppedByYou: "You stopped a background command",
  jobStopped: "Background command stopped",
  jobInterrupted: "Background command interrupted",
  jobCouldNotRun: "Background command couldn't run",
  answered: (label, took) => (took ? `${label} handed back · ${took}` : `${label} handed back`),
  answerFailed: (label) => `${label}: turn failed`,
  watchEnded: () => "Sync stopped",
  watchInterrupted: () => "Disconnected, reconnecting…",
  notSent: () => "Couldn't send",
  health: {
    reconnecting: "Connection lost, retrying",
    safety: "OpenAI is giving this turn's output extra safety review",
    rerouted: "OpenAI moved this turn to another model",
    tools_missing: "The World's tools are not connected",
    tool_server: "A set of tools did not start",
    warning: "A notice",
    error: "This turn hit an error",
  },
  files: {
    reveal: "Show in folder",
    copyPath: "Copy path",
    copied: "Path copied",
    on: (machine, path) => `${path} on ${machine}`,
    openFailed: (why) => `Couldn't open the file: ${why}`,
    gone: "The original is gone",
  },
  pictures: {
    close: "Close",
    previous: "Previous",
    next: "Next",
    copyImage: "Copy image",
    saveAs: "Save as",
    copyPath: "Copy path",
    reveal: "Show in folder",
    copied: "Copied",
    saved: "Saved",
    gone: "The original is gone",
    failed: (why) => `Couldn't get it: ${why}`,
    counter: (at, of) => `${at} / ${of}`,
  },
  openColleague: "Open",
  subagent: (kind, steps, doing) => [`Subagent${kind ? ` ${kind}` : ""}`, `${steps} ${steps === 1 ? "step" : "steps"}`, doing].filter(Boolean).join(" · "),
  stillRunning: "Running commands",
  stopJobAsk: "Stop this command? The agent will be told you stopped it.",
  notStopped: "Couldn't stop it",
  cancel: "Cancel",
  jobOutput: (command) => `Output of ${command}`,
  jobOutputLoading: "Reading…",
  jobOutputFailed: (why) => `Couldn't read the output: ${why}`,
  jobOver: "Ended",
  jobRunning: (job, machine, lasted) => `${job} · ${machine} · ${lasted}`,
  minutes: (n) => (n < 1 ? "under a minute" : `${n} min`),
  hours: (hours, minutes) => (minutes ? `${hours} h ${minutes} min` : `${hours} h`),
  landed: (how, machine) =>
    how === "sent"
      ? `Sent to ${machine}`
      : how === "uncertain"
        ? "Not sure it arrived"
        : "Arrives when it next uses a World tool",
  jobEnded: (_job, machine) => `Background command on ${machine} ended`,
  movedTo: (town) => `Moved to project ${town}`,
  movedOut: "Removed from project",
  briefHanded: "Project brief",
  deskFree: (who) => `${who}'s turn ended; the screen is free`,
  deskTaken: (who) => `${who} is using the screen; you'll be told when it's free`,
  deskOutside: "Computer Use is held by a session outside the World",
  ended: (how) =>
    endedIn(how, {
      exited: (code) => `exit ${code}`,
      signal: (signal) => `ended by signal ${signal}`,
      stopped: "stopped",
      finished: "finished",
      interrupted: (reason) => `interrupted: ${reason}`,
      couldNotRun: (error) => `could not run: ${error}`,
      turn: (how, reason) => `turn ${how}${reason ? `: ${reason}` : ""}`,
      inTurn: "in its turn",
    }),
};

interface EndedWords {
  exited(code: string): string;
  signal(signal: string): string;
  stopped: string;
  finished: string;
  interrupted(reason: string): string;
  couldNotRun(error: string): string;
  turn(how: string, reason: string | null): string;
  inTurn: string;
}

/** Reads the World's wording of how something ended, and says it again;
 *  a trailing duration (`, 14m`) is kept as it is. */
function endedIn(how: string, say: EndedWords): string {
  const timed = /^(.*), (\d+[smhd](?:\d+[mh])?)$/.exec(how);
  const what = timed ? timed[1] : how;
  const took = timed ? ` · ${timed[2]}` : "";
  let said: string;
  let match: RegExpExecArray | null;
  if ((match = /^exited (\S+)$/.exec(what))) said = say.exited(match[1]);
  else if ((match = /^ended by signal (\d+)$/.exec(what))) said = say.signal(match[1]);
  else if (what === "stopped") said = say.stopped;
  else if (what === "finished") said = say.finished;
  else if ((match = /^interrupted: (.*)$/.exec(what))) said = say.interrupted(match[1]);
  else if ((match = /^could not run: (.*)$/.exec(what))) said = say.couldNotRun(match[1]);
  else if (what === "in its turn") said = say.inTurn;
  else if ((match = /^turn (\w+)(?:: (.*))?$/.exec(what))) said = say.turn(match[1], match[2] ?? null);
  else said = what;
  return said + took;
}
