import type { HealthKind } from "./world/events";
import { dollars, underCent } from "./world/money";
import { enProblems, known, zhProblems, type Reached } from "./app/problem";
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
  | "copy"
  | "search"
  | "image"
  | "ask"
  | "message"
  | "fetch"
  | "websearch"
  | "tasks"
  /** The host's own agents: starting one, waiting for some, stopping one,
   *  bringing one back, listing them. */
  | "delegate"
  | "wait"
  | "stop"
  | "resume"
  | "list";

/** One kind of thing a stretch did, said: "修改 5 个文件". The files it
 *  changed carry the lines added and removed after them; a failure is
 *  coloured as one. */
export interface TallyPart {
  said: string;
  kind: "edited" | "done" | "failed";
}

export interface Tally {
  read: number;
  edited: number;
  ran: number;
  /** Files copied between machines. */
  copied: number;
  searched: number;
  asked: number;
  /** The host's own agents it started. */
  delegated: number;
  /** What it said to its agents, waited on them for, stopped or resumed. */
  coordinated: number;
  other: number;
  failed: number;
  thinking: number;
}

/** The host's own agents, as a conversation shows them (`agents.ts`). */
export interface AgentWords {
  /** How one stands where its row does not say it by its mark. */
  failed: string;
  stopped: string;
  waiting: string;
  /** What it was asked, at the head of what it did, opened in full. */
  more: string;
  less: string;
  /** Its own record, read from its conversation's machine. */
  reading: string;
  unreadable: string;
  /** It has done nothing yet. */
  nothing: string;
  /** Its end, as the conversation was told of it. */
  done(task: string | null): string;
  failedTold(task: string | null): string;
  stoppedTold(task: string | null): string;
  /** What it used, at the foot of what it did. */
  tools(n: number): string;
  /** A call that names agents the conversation does not hold. */
  unknownAgents(n: number): string;
}

/** The rows of what a run did through the World. */
export interface TripWords {
  /** One command sent to `n` machines. */
  machines(n: number): string;
  /** How many of them did not end as they should. */
  troubled(n: number): string;
  command: string;
  lastLines: string;
  stopped: string;
  lost(why: string): string;
  /** A command whose machine went away under it, as its row says it. */
  unfinished: string;
  /** Under the pointer on the hook of a command whose input flowed
   *  straight from the row above. */
  fed: string;
  from: string;
  to: string;
  copied(files: number, size: string): string;
  problems: string;
}

export interface Words {
  english: boolean;
  queue: QueueWords;
  /** A card of questions asked without waiting. */
  questions: QuestionWords;
  act(act: Act, tense: Tense): string;
  failedTool(name: string): string;
  /** What a finished stretch did, by kind: its heading's parts in the order
   *  read, set apart by `tallyGap`. */
  /** What a stretch did, kind by kind, the weightiest first: files changed,
   *  commands run, agents and models called, searches, reads; then how
   *  many failed. */
  tally(tally: Tally): TallyPart[];
  tallyGap: string;
  /** Thinking under way, as both hosts say it, in every language. */
  thinking: string;
  /** Thinking under way, on the section its words are now under. */
  thinkingAbout(title: string): string;
  /** Thinking that is over, its length not told. */
  thought: string;
  /** Thinking that is over: "Thought for 4s". */
  thoughtFor(ms: number): string;
  /** The tokens a run of thinking took: "1.2k tokens". */
  thinkingTokens(n: number): string;
  plan: string;
  copyMessage: string;
  /** A long message of the user's, its first lines shown: the rest, and
   *  back. */
  bubbleMore: string;
  bubbleLess: string;
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
  /** A command the user gave the conversation: while it runs; one that
   *  did not go through, its words in the hover; one whose host did not say
   *  what came of it; one that ran with nothing else to show; a rewind,
   *  and whether the files came back with it; a fork that left some of it
   *  behind. */
  commandRunning(name: string): string;
  commandFailed(name: string): string;
  commandUnsure(name: string): string;
  commandRan(name: string): string;
  rewound(files: boolean): string;
  forkPartial: string;
  /** Turns not held, before everything held and between two parts. */
  turnsEarlier(turns: number): string;
  turnsBetween(turns: number): string;
  /** Back to the newest, from far up a conversation. */
  toLatest: string;
  /** A long stretch's earlier rows, not shown yet. */
  stepsEarlier(steps: number): string;
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
  /** A change's card: its whole as a unified diff. */
  copyDiff: string;
  sendPlaceholder(agent: string): string;
  send: string;
  stop: string;
  /** The stop button while the turn it stops has not ended. */
  stopping: string;
  /** A stop that could not be asked for. */
  turnNotStopped(why: string): string;
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
  idle: string;
  /** The user archived it: kept to read, not to speak to. */
  removed: string;
  removedNote: string;
  waitingForYou: string;
  on(machine: string): string;
  /** Under the title of a conversation with an agent assisting the user. */
  assistFrom(owner: string): string;
  /** Between the names in a short list. */
  listSeparator: string;
  /** Between figures said side by side: a step count and a time. */
  figureGap: string;
  /** The button under the first lines of something long: "+ 28 行". */
  moreLines(n: number): string;
  unchangedLines(n: number): string;
  /** Long text opened whole, in a popup. */
  fullScreen: string;
  /** What a command printed and what a write changed, opened under their
   *  rows: what was left out of what the model got, the whole output on
   *  its machine beside it, and what is said when either is gone. */
  /** What a run did through the World, row by row (`conversation/trip.ts`). */
  trip: TripWords;
  output: OutputWords & {
    /** What a call was given, and what it gave back, as their boxes are named. */
    input: string;
    printed: string;
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
   *  reported, how long, the tokens, what it cost by the host's own ledger,
   *  and the login's windows as the turn left them. */
  turnInfo: {
    model: string;
    effort: string;
    requested(effort: string): string;
    took: string;
    tokens: string;
    spent(input: number, cached: number, output: number, reasoning: number): string;
    cost: string;
    costOf(usd: number): string;
    limits: string;
    window(name: string, minutes: number | null): string;
    /** The turn's requests one by one, on its login's usage page. */
    requests: string;
  };
  /** How many steps a stretch of work has taken. */
  steps(n: number): string;
  /** Under a turn's last message: copy all it said, ask the same again. */
  copyAnswer: string;
  regenerate: string;
  /** A colleague handed back its answer: "GPT 交回　14 分钟". */
  answered(label: string, took: string | null): string;
  answerFailed(label: string): string;
  /** The watch on the session ended: the node ended it, or the connection
   *  to it broke. */
  watchEnded(why: string | null): string;
  /** Its machine refused this account the conversation. */
  watchRefused: string;
  /** The live stream broke and is waiting to be read again. */
  watchInterrupted(why: string | null): string;
  /** On hover over that: for how long, and why. */
  brokenFor(took: string, why: string | null): string;
  /** Words that could not be sent. */
  notSent(why: string): string;
  /** Words with no answer yet, said again until one comes. */
  waitingToSend: string;
  /** What the session's host said of its own health, in a plain line; the
   *  host's words go on hover. */
  health: Record<HealthKind, string>;
  /** An assistance's allowance reached, and until when: a turn stopped
   *  for it says so. */
  allowanceUsed(reached: Reached): string;
  /** Links an agent writes to files. */
  files: FileLinkWords;
  /** The conversation's pictures, small and over the whole window. */
  pictures: PictureWords;
  /** The way into a colleague's own conversation. */
  openColleague: string;
  /** The host's own agents. */
  agents: AgentWords;
  /** What it started that still runs. */
  stillRunning: string;
  /** Before a job is stopped by the user, and when it could not be. */
  stopJobAsk: string;
  /** A sentinel the conversation left: withdrawing it, and how it stands. */
  withdrawSentinel: string;
  withdrawSentinelAsk: string;
  sentinelPhase(phase: "starting" | "watching" | "satisfied" | "unable" | "withdrawn" | "unavailable"): string;
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
  /** The machines and folders a conversation is pointed at, changed. */
  focusChanged(items: string): string;
  focusCleared: string;
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
  copy: ["复制", "复制中", "复制失败"],
  search: ["搜索", "搜索中", "搜索失败"],
  image: ["看图", "查看中", "查看失败"],
  ask: ["调用", "调用中", "调用失败"],
  message: ["发送", "发送中", "发送失败"],
  fetch: ["获取", "获取中", "获取失败"],
  websearch: ["网页搜索", "搜索中", "搜索失败"],
  tasks: ["待办", "更新中", "更新失败"],
  delegate: ["委派", "委派中", "委派失败"],
  wait: ["等待", "等待中", "等待失败"],
  stop: ["停止", "停止中", "停止失败"],
  resume: ["唤回", "唤回中", "唤回失败"],
  list: ["查看", "查看中", "查看失败"],
};

const enActs: Record<Act, [string, string, string]> = {
  read: ["Read", "Reading", "Read Failed"],
  edit: ["Edited", "Editing", "Edit Failed"],
  create: ["Created", "Creating", "Create Failed"],
  delete: ["Deleted", "Deleting", "Delete Failed"],
  move: ["Moved", "Moving", "Move Failed"],
  write: ["Wrote", "Writing", "Write Failed"],
  command: ["Ran", "Running command", "Command Failed"],
  copy: ["Copied", "Copying", "Copy Failed"],
  search: ["Searched", "Searching", "Search Failed"],
  image: ["Viewed", "Viewing image", "Image Failed"],
  ask: ["Called", "Calling a model", "Call Failed"],
  message: ["Sent", "Writing to a model", "Message Failed"],
  fetch: ["Fetched", "Fetching", "Fetch Failed"],
  websearch: ["Searched the web", "Searching the web", "Web search Failed"],
  tasks: ["Tasks", "Updating tasks", "Tasks Failed"],
  delegate: ["Delegated", "Delegating", "Delegation Failed"],
  wait: ["Waited", "Waiting", "Wait Failed"],
  stop: ["Stopped", "Stopping", "Stop Failed"],
  resume: ["Resumed", "Resuming", "Resume Failed"],
  list: ["Listed", "Listing", "List Failed"],
};

const index = (tense: Tense) => (tense === "done" ? 0 : tense === "running" ? 1 : 2);

export const zh: Words = {
  english: false,
  questions: zhQuestions,
  queue: {
    waiting: (count) => `排队 ${count} 条`,
    held: "已暂停，本轮停止",
    notTaken: "已暂停，上一条没送出",
    unsure: "不知是否送到",
    sendNow: "引导",
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
    const parts: TallyPart[] = [];
    if (t.edited) parts.push({ said: `修改 ${t.edited} 个文件`, kind: "edited" });
    if (t.ran) parts.push({ said: `运行 ${t.ran} 条命令`, kind: "done" });
    if (t.copied) parts.push({ said: `复制 ${t.copied} 个文件`, kind: "done" });
    if (t.delegated) parts.push({ said: `委派 ${t.delegated} 个子代理`, kind: "done" });
    if (t.asked) parts.push({ said: `调用模型 ${t.asked} 次`, kind: "done" });
    if (t.searched) parts.push({ said: `搜索 ${t.searched} 次`, kind: "done" });
    if (t.read) parts.push({ said: `读取 ${t.read} 个文件`, kind: "done" });
    if (t.coordinated) parts.push({ said: `调度子代理 ${t.coordinated} 次`, kind: "done" });
    if (t.other) parts.push({ said: `调用 ${t.other} 个工具`, kind: "done" });
    if (!parts.length) parts.push({ said: t.thinking ? "Thought" : "活动", kind: "done" });
    if (t.failed) parts.push({ said: `${t.failed} 个失败`, kind: "failed" });
    return parts;
  },
  tallyGap: "　",
  thinking: "Thinking…",
  thinkingAbout: (title) => `Thinking… ${title}`,
  thought: "Thought",
  thoughtFor: (ms) => `Thought for ${clock(ms)}`,
  thinkingTokens: (n) => tokensSaid(n),
  plan: "计划",
  copyMessage: "复制",
  bubbleMore: "展开全文",
  bubbleLess: "收起",
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
  you: (machine) => `来自 ${machine}`,
  world: "Fastctx",
  aPage: (name) => `页面「${name}」`,
  compacted: "上下文已压缩",
  commandRunning: (name) =>
    ({ compact: "正在压缩…", review: "正在审查…", fork: "正在分叉…", rewind: "正在回退…", rename: "正在改名…", model: "正在换模型…" })[name] ?? `正在运行 /${name}…`,
  commandFailed: (name) => `/${name} 没成`,
  commandUnsure: (name) => `/${name} 不确定做成了没有`,
  commandRan: (name) => `/${name} 已执行`,
  rewound: (files) => (files ? "已回到这一轮之前" : "已回到这一轮之前，改过的文件没有还原"),
  forkPartial: "已分叉，部分内容没带过去",
  turnsEarlier: (turns) => (turns > 0 ? `更早的 ${turns} 轮` : "更早的记录"),
  turnsBetween: (turns) => (turns > 0 ? `中间还有 ${turns} 轮` : "中间还有记录"),
  toLatest: "回到最新",
  stepsEarlier: (steps) => `更早的 ${steps} 步`,
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
  copyDiff: "复制改动",
  sendPlaceholder: (agent) => `发消息给 ${agent}…`,
  send: "发送",
  stop: "停止",
  stopping: "正在停",
  turnNotStopped: () => "没能停下",
  uncertain: () => "送达状态未知",
  sending: "发送中",
  context: (percent) => `上下文 ${percent}%`,
  working: "工作中",
  starting: "启动中",
  startFailed: "启动失败",
  idle: "空闲",
  removed: "已归档",
  removedNote: "已归档，对话仅供查看",
  waitingForYou: "等待回复",
  on: (machine) => machine,
  assistFrom: (owner) => `来自 ${owner}`,
  listSeparator: "、",
  figureGap: "　",
  moreLines: (n) => `+ ${n.toLocaleString("zh-CN")} 行`,
  unchangedLines: (n) => `${n} 行未改动`,
  fullScreen: "查看全部",
  trip: {
    machines: (n) => `${n} 台`,
    troubled: (n) => `${n} 台出错`,
    command: "命令",
    lastLines: "最后几行",
    stopped: "已停止",
    lost: (why) => `没有跑完：${why}`,
    unfinished: "没有跑完",
    fed: "用上一行的输出",
    from: "从",
    to: "到",
    copied: (files, size) => `${files} 个文件，${size}`,
    problems: "没复制成的",
  },
  output: {
    input: "输入",
    printed: "输出",
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
        .join("，"),
    cost: "花费",
    costOf: (usd) => (underCent(usd) ? "不到 $0.01" : `约 ${dollars(usd)}`),
    limits: "额度",
    window: (name, minutes) => (minutes === 300 || name.startsWith("five_hour") ? "5 小时" : minutes === 10080 || name.startsWith("seven_day") ? "7 天" : name),
    requests: "查看每个请求",
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
  answered: (label, took) => (took ? `${label} 交回　${took}` : `${label} 交回`),
  answerFailed: (label) => `${label} 本轮失败`,
  watchEnded: () => "已停止同步",
  watchRefused: "你的账号看不了这段对话",
  watchInterrupted: () => "连接中断，正在重连…",
  brokenFor: (took, why) => (why ? `已断开 ${took}：${why}` : `已断开 ${took}`),
  notSent: (why) => known(why, zhProblems) ?? "发送失败",
  waitingToSend: "等待发送",
  health: {
    reconnecting: "连接中断，正在重试",
    safety: "OpenAI 正在对这一轮的输出做额外的安全审查",
    rerouted: "OpenAI 把这一轮换到了另一个模型",
    tools_missing: "Fastctx 的工具没有接上",
    tool_server: "有一组工具没能启动",
    warning: "有一条提醒",
    error: "这一轮出错了",
  },
  allowanceUsed: zhProblems.allowanceUsed,
  files: {
    reveal: "在文件夹中显示",
    copyPath: "复制路径",
    copied: "已复制路径",
    on: (machine, path) => `${machine} 上的 ${path}`,
    openFailed: (why) => `打不开这个文件：${why}`,
    gone: "原件已不在",
    fetching: (name, share) => (share === null ? `正在取 ${name}` : `正在取 ${name}　${share}%`),
    fetchInterrupted: (name) => `${name}：连接中断，正在重连…`,
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
    fetching: (share) => (share === null ? "正在取…" : `正在取… ${share}%`),
    interrupted: "连接中断，正在重连…",
  },
  openColleague: "查看",
  agents: {
    failed: "失败",
    stopped: "已停止",
    waiting: "在等",
    more: "展开",
    less: "收起",
    reading: "正在读取…",
    unreadable: "读不到它的记录",
    nothing: "还没开始",
    done: (task) => (task ? `子代理已完成：${task}` : "子代理已完成"),
    failedTold: (task) => (task ? `子代理失败：${task}` : "子代理失败"),
    stoppedTold: (task) => (task ? `子代理已停止：${task}` : "子代理已停止"),
    tools: (n) => `${n} 次工具调用`,
    unknownAgents: (n) => `${n} 个子代理`,
  },
  stillRunning: "还在跑的",
  stopJobAsk: "停下这条命令？会告诉智能体是你停的。",
  withdrawSentinel: "撤掉",
  withdrawSentinelAsk: "撤掉这个哨兵？",
  sentinelPhase: (phase) =>
    ({ starting: "刚开始", watching: "盯着", satisfied: "已成立", unable: "盯不成", withdrawn: "已撤掉", unavailable: "暂时问不到" })[phase],
  notStopped: "没能停下",
  cancel: "取消",
  jobOutput: (command) => `${command} 的输出`,
  jobOutputLoading: "正在读取…",
  jobOutputFailed: (why) => `读不到输出：${why}`,
  jobOver: "已结束",
  jobRunning: (job, machine, lasted) => `${job}　${machine}　${lasted}`,
  minutes: (n) => (n < 1 ? "不到 1 分钟" : `${n} 分钟`),
  hours: (hours, minutes) => (minutes ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`),
  // Words that wait for a known reason say it, as a refused turn's do.
  landed: (how, machine, why) =>
    how === "waiting" && why && known(why, zhProblems)
      ? known(why, zhProblems)!
      : how === "sent"
      ? `已发往 ${machine}`
      : how === "uncertain"
        ? "不确定是否送到"
        : how === "kept"
          ? "排着，等这一轮结束"
          : "等它下次用世界的工具时送到",
  jobEnded: (_job, machine) => `后台命令在 ${machine} 上结束`,
  movedTo: (town) => `已移到项目 ${town}`,
  movedOut: "已移出项目",
  focusChanged: (items) => `机器与文件夹：${items}`,
  focusCleared: "机器与文件夹：未选",
  briefHanded: "项目说明",
  deskFree: (who) => `${who} 这一轮结束，屏幕空出来了`,
  deskTaken: (who) => `${who} 正在操作屏幕，结束后会通知`,
  deskOutside: "屏幕被 Fastctx 之外的会话占着",
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
    held: "Paused, turn stopped",
    notTaken: "Paused, last one not sent",
    unsure: "Delivery unknown",
    sendNow: "Guide",
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
    const parts: TallyPart[] = [];
    if (t.edited) parts.push({ said: `edited ${plural(t.edited, "file", "files")}`, kind: "edited" });
    if (t.ran) parts.push({ said: `ran ${plural(t.ran, "command", "commands")}`, kind: "done" });
    if (t.copied) parts.push({ said: `copied ${plural(t.copied, "file", "files")}`, kind: "done" });
    if (t.delegated) parts.push({ said: `delegated ${plural(t.delegated, "agent", "agents")}`, kind: "done" });
    if (t.asked) parts.push({ said: `called ${plural(t.asked, "model", "models")}`, kind: "done" });
    if (t.searched) parts.push({ said: t.searched === 1 ? "searched once" : `searched ${t.searched} times`, kind: "done" });
    if (t.read) parts.push({ said: `read ${plural(t.read, "file", "files")}`, kind: "done" });
    if (t.coordinated) parts.push({ said: t.coordinated === 1 ? "coordinated agents once" : `coordinated agents ${t.coordinated} times`, kind: "done" });
    if (t.other) parts.push({ said: plural(t.other, "tool call", "tool calls"), kind: "done" });
    if (!parts.length) parts.push({ said: t.thinking ? "Thought" : "Activity", kind: "done" });
    parts[0] = { ...parts[0]!, said: parts[0]!.said.charAt(0).toUpperCase() + parts[0]!.said.slice(1) };
    if (t.failed) parts.push({ said: `${t.failed} failed`, kind: "failed" });
    return parts;
  },
  tallyGap: ", ",
  thinking: "Thinking…",
  thinkingAbout: (title) => `Thinking… ${title}`,
  thought: "Thought",
  thoughtFor: (ms) => `Thought for ${clock(ms)}`,
  thinkingTokens: (n) => tokensSaid(n),
  plan: "Plan",
  copyMessage: "Copy",
  bubbleMore: "Show all",
  bubbleLess: "Show less",
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
  you: (machine) => `From ${machine}`,
  world: "Fastctx",
  aPage: (name) => `the page “${name}”`,
  compacted: "Context compacted",
  commandRunning: (name) =>
    ({ compact: "Compacting…", review: "Reviewing…", fork: "Forking…", rewind: "Rewinding…", rename: "Renaming…", model: "Changing model…" })[name] ?? `Running /${name}…`,
  commandFailed: (name) => `/${name} didn't go through`,
  commandUnsure: (name) => `Not sure whether /${name} went through`,
  commandRan: (name) => `/${name} ran`,
  rewound: (files) => (files ? "Back to before this turn" : "Back to before this turn; changed files were not restored"),
  forkPartial: "Forked; part of it did not come over",
  turnsEarlier: (turns) => (turns === 1 ? "1 earlier turn" : turns > 0 ? `${turns} earlier turns` : "Earlier turns"),
  turnsBetween: (turns) => (turns === 1 ? "1 more turn between" : turns > 0 ? `${turns} more turns between` : "More turns between"),
  toLatest: "Back to latest",
  stepsEarlier: (steps) => (steps === 1 ? "1 earlier step" : `${steps} earlier steps`),
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
  copyDiff: "Copy diff",
  sendPlaceholder: (agent) => `Message ${agent}…`,
  send: "Send",
  stop: "Stop",
  stopping: "Stopping",
  turnNotStopped: () => "Couldn't stop",
  uncertain: () => "Delivery unknown",
  sending: "Sending",
  context: (percent) => `Context ${percent}%`,
  working: "Working",
  starting: "Starting",
  startFailed: "Couldn't start",
  idle: "Idle",
  removed: "Archived",
  removedNote: "Archived; the conversation is read-only",
  waitingForYou: "Waiting for your reply",
  on: (machine) => machine,
  assistFrom: (owner) => `from ${owner}`,
  listSeparator: ", ",
  figureGap: " ",
  moreLines: (n) => `+ ${n.toLocaleString("en")} more line${n === 1 ? "" : "s"}`,
  unchangedLines: (n) => `${n} unchanged lines`,
  fullScreen: "View all",
  trip: {
    machines: (n) => `${n} machines`,
    troubled: (n) => `${n} failed`,
    command: "Command",
    lastLines: "Last lines",
    stopped: "Stopped",
    lost: (why) => `Did not finish: ${why}`,
    unfinished: "Did not finish",
    fed: "Takes the output of the row above",
    from: "From",
    to: "To",
    copied: (files, size) => `${files} ${files === 1 ? "file" : "files"}, ${size}`,
    problems: "Not copied",
  },
  output: {
    input: "Input",
    printed: "Output",
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
        .join(", "),
    cost: "Cost",
    costOf: (usd) => (underCent(usd) ? "under $0.01" : `≈ ${dollars(usd)}`),
    limits: "Limits",
    window: (name, minutes) => (minutes === 300 || name.startsWith("five_hour") ? "5 hours" : minutes === 10080 || name.startsWith("seven_day") ? "7 days" : name),
    requests: "View requests",
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
  answered: (label, took) => (took ? `${label} handed back after ${took}` : `${label} handed back`),
  answerFailed: (label) => `${label}: turn failed`,
  watchEnded: () => "Sync stopped",
  watchRefused: "Your account can't see this conversation",
  watchInterrupted: () => "Disconnected, reconnecting…",
  brokenFor: (took, why) => (why ? `Disconnected for ${took}: ${why}` : `Disconnected for ${took}`),
  notSent: (why) => known(why, enProblems) ?? "Couldn't send",
  waitingToSend: "Waiting to send",
  health: {
    reconnecting: "Connection lost, retrying",
    safety: "OpenAI is giving this turn's output extra safety review",
    rerouted: "OpenAI moved this turn to another model",
    tools_missing: "Fastctx's tools are not connected",
    tool_server: "A set of tools did not start",
    warning: "A notice",
    error: "This turn hit an error",
  },
  allowanceUsed: enProblems.allowanceUsed,
  files: {
    reveal: "Show in folder",
    copyPath: "Copy path",
    copied: "Path copied",
    on: (machine, path) => `${path} on ${machine}`,
    openFailed: (why) => `Couldn't open the file: ${why}`,
    gone: "The original is gone",
    fetching: (name, share) => (share === null ? `Fetching ${name}` : `Fetching ${name}, ${share}%`),
    fetchInterrupted: (name) => `${name}: disconnected, reconnecting…`,
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
    fetching: (share) => (share === null ? "Fetching…" : `Fetching… ${share}%`),
    interrupted: "Disconnected, reconnecting…",
  },
  openColleague: "Open",
  agents: {
    failed: "failed",
    stopped: "stopped",
    waiting: "waiting",
    more: "Show all",
    less: "Show less",
    reading: "Reading…",
    unreadable: "Its record can't be read",
    nothing: "Not started yet",
    done: (task) => (task ? `Agent finished: ${task}` : "Agent finished"),
    failedTold: (task) => (task ? `Agent failed: ${task}` : "Agent failed"),
    stoppedTold: (task) => (task ? `Agent stopped: ${task}` : "Agent stopped"),
    tools: (n) => (n === 1 ? "1 tool call" : `${n} tool calls`),
    unknownAgents: (n) => (n === 1 ? "1 agent" : `${n} agents`),
  },
  stillRunning: "Still under way",
  stopJobAsk: "Stop this command? The agent will be told you stopped it.",
  withdrawSentinel: "Withdraw",
  withdrawSentinelAsk: "Withdraw this sentinel?",
  sentinelPhase: (phase) =>
    ({ starting: "starting", watching: "watching", satisfied: "found it", unable: "can't watch", withdrawn: "withdrawn", unavailable: "can't be asked now" })[phase],
  notStopped: "Couldn't stop it",
  cancel: "Cancel",
  jobOutput: (command) => `Output of ${command}`,
  jobOutputLoading: "Reading…",
  jobOutputFailed: (why) => `Couldn't read the output: ${why}`,
  jobOver: "Ended",
  jobRunning: (job, machine, lasted) => `${job} on ${machine}, ${lasted}`,
  minutes: (n) => (n < 1 ? "under a minute" : `${n} min`),
  hours: (hours, minutes) => (minutes ? `${hours} h ${minutes} min` : `${hours} h`),
  landed: (how, machine, why) =>
    how === "waiting" && why && known(why, enProblems)
      ? known(why, enProblems)!
      : how === "sent"
      ? `Sent to ${machine}`
      : how === "uncertain"
        ? "Not sure it arrived"
        : how === "kept"
          ? "Queued until this turn ends"
          : "Arrives when it next uses a Fastctx tool",
  jobEnded: (_job, machine) => `Background command on ${machine} ended`,
  movedTo: (town) => `Moved to project ${town}`,
  movedOut: "Removed from project",
  focusChanged: (items) => `Machines and folders: ${items}`,
  focusCleared: "Machines and folders: none",
  briefHanded: "Project brief",
  deskFree: (who) => `${who}'s turn ended; the screen is free`,
  deskTaken: (who) => `${who} is using the screen; you'll be told when it's free`,
  deskOutside: "Computer Use is held by a session outside Fastctx",
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
  const took = timed ? `　${timed[2]}` : "";
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

/** A span as the hosts write it after "Thought for": 4s, 1m 4s, 1h 2m;
 *  never under a second. */
function clock(ms: number): string {
  const seconds = Math.max(1, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return seconds % 60 ? `${minutes}m ${seconds % 60}s` : `${minutes}m`;
  return minutes % 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${Math.floor(minutes / 60)}h`;
}

/** Tokens as the hosts count them in a line: 640 tokens, 1.2k tokens. */
function tokensSaid(n: number): string {
  if (n === 1) return "1 token";
  if (n < 1000) return `${n} tokens`;
  const short = (value: number, unit: string) => `${value >= 100 ? Math.round(value) : Number(value.toFixed(1))}${unit} tokens`;
  return n < 1_000_000 ? short(n / 1000, "k") : short(n / 1_000_000, "M");
}
