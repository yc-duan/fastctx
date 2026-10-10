import type { QueueWords } from "./conversation/queue";
// Every word the interface says on its own account, in the user's words
// (`world-gui.md`, 用词): plain, without role-play; what agents and tools
// produce is never translated.

import type { FindWords } from "./conversation/find";
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
  /** How long something took: "1 分 12 秒". */
  took(ms: number): string;
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
  /** The way into a colleague's own conversation. */
  openColleague: string;
  /** What it started that still runs. */
  stillRunning: string;
  /** Before a job is stopped by the user, and when it could not be. */
  stopJobAsk: string;
  notStopped: string;
  cancel: string;
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
  command: ["运行命令", "运行中", "命令失败"],
  search: ["搜索", "搜索中", "搜索失败"],
  image: ["查看图片", "查看中", "查看失败"],
  ask: ["请同事", "请同事中", "请同事失败"],
  message: ["发消息给同事", "发送中", "发送失败"],
  fetch: ["获取网页", "获取中", "获取失败"],
  websearch: ["网页搜索", "搜索中", "搜索失败"],
  tasks: ["更新待办", "更新中", "更新失败"],
};

const enActs: Record<Act, [string, string, string]> = {
  read: ["Read", "Reading", "Read Failed"],
  edit: ["Edited", "Editing", "Edit Failed"],
  create: ["Created", "Creating", "Create Failed"],
  delete: ["Deleted", "Deleting", "Delete Failed"],
  move: ["Moved", "Moving", "Move Failed"],
  write: ["Wrote", "Writing", "Write Failed"],
  command: ["Ran command", "Running command", "Command Failed"],
  search: ["Searched", "Searching", "Search Failed"],
  image: ["Viewed image", "Viewing image", "Image Failed"],
  ask: ["Asked a colleague", "Asking a colleague", "Colleague Failed"],
  message: ["Wrote to a colleague", "Writing to a colleague", "Message Failed"],
  fetch: ["Fetched", "Fetching", "Fetch Failed"],
  websearch: ["Searched the web", "Searching the web", "Web search Failed"],
  tasks: ["Updated tasks", "Updating tasks", "Tasks Failed"],
};

const index = (tense: Tense) => (tense === "done" ? 0 : tense === "running" ? 1 : 2);

export const zh: Words = {
  english: false,
  questions: zhQuestions,
  queue: {
    waiting: (count) => `${count} 条待发送，本轮结束后依次发出`,
    held: "本轮已停止，队列暂停",
    notTaken: "上一条发送失败，队列暂停",
    unsure: "上一条送达状态未知，队列暂停",
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
  },
  act: (act, tense) => zhActs[act][index(tense)],
  failedTool: (name) => `${name} 失败`,
  tally: (t) => {
    const parts: string[] = [];
    if (t.read) parts.push(`读取 ${t.read} 个文件`);
    if (t.edited) parts.push(`修改 ${t.edited} 个文件`);
    if (t.ran) parts.push(`运行 ${t.ran} 条命令`);
    if (t.searched) parts.push(`搜索 ${t.searched} 次`);
    if (t.asked) parts.push(`请 ${t.asked} 位同事`);
    if (t.other) parts.push(`调用 ${t.other} 个工具`);
    let heading = parts.join(" · ");
    if (!heading) heading = t.thinking ? "思考" : "活动";
    return t.failed ? `${heading} · ${t.failed} 个失败` : heading;
  },
  thought: "思考",
  thinking: "正在思考",
  thoughtWithheld: "思考内容未公开",
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
  exited: (code) => `退出码 ${code}`,
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
  answering: "书写回答",
  idle: "空闲",
  removed: "已移除",
  removedNote: "已移除，对话仅供查看",
  waitingForYou: "等待回复",
  on: (machine) => machine,
  worksOn: (machine) => `工作于 ${machine}`,
  lentBy: (owner) => `借自 ${owner}`,
  listSeparator: "、",
  moreLines: (n) => `+ ${n.toLocaleString("zh-CN")} 行`,
  unchangedLines: (n) => `${n} 行未改动`,
  fullScreen: "整屏查看",
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
  openColleague: "查看",
  stillRunning: "运行中的命令",
  stopJobAsk: "停下这条命令？会告诉智能体是你停的。",
  notStopped: "没能停下",
  cancel: "取消",
  jobRunning: (job, machine, lasted) => `${job} · ${machine} · ${lasted}`,
  minutes: (n) => (n < 1 ? "不到 1 分钟" : `${n} 分钟`),
  hours: (hours, minutes) => (minutes ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`),
  landed: (how, machine) =>
    how === "sent"
      ? `已发往 ${machine}，送达后投递`
      : how === "uncertain"
        ? "送达状态未知"
        : "将在它下次调用 fastctx 时送达",
  jobEnded: (job, machine) => `作业 ${job} 在 ${machine} 上结束`,
  movedTo: (town) => `已移到项目 ${town}`,
  movedOut: "已移出项目",
  briefHanded: "项目说明",
  deskFree: (who) => `${who} 本轮结束，Computer Use 已空闲`,
  deskTaken: (who) => `${who} 正在使用 Computer Use，结束后会通知`,
  deskOutside: "Computer Use 被世界之外的会话占用",
  ended: (how) =>
    endedIn(how, {
      exited: (code) => `退出码 ${code}`,
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
    waiting: (count) => `${count} queued, sent in order after this turn`,
    held: "Turn stopped; queue paused",
    notTaken: "Last message failed; queue paused",
    unsure: "Last message delivery unknown; queue paused",
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
    if (t.asked) parts.push(`asked ${plural(t.asked, "colleague", "colleagues")}`);
    if (t.other) parts.push(plural(t.other, "tool call", "tool calls"));
    let heading = parts.join(", ");
    if (!heading) heading = t.thinking ? "Thinking" : "Activity";
    heading = heading.charAt(0).toUpperCase() + heading.slice(1);
    return t.failed ? `${heading} · ${t.failed} failed` : heading;
  },
  thought: "Thought",
  thinking: "Thinking",
  thoughtWithheld: "Thinking not shared",
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
  answering: "Writing",
  idle: "Idle",
  removed: "Removed",
  removedNote: "Removed; the conversation is read-only",
  waitingForYou: "Waiting for your reply",
  on: (machine) => machine,
  worksOn: (machine) => `works on ${machine}`,
  lentBy: (owner) => `lent by ${owner}`,
  listSeparator: ", ",
  moreLines: (n) => `+ ${n.toLocaleString("en")} more line${n === 1 ? "" : "s"}`,
  unchangedLines: (n) => `${n} unchanged lines`,
  fullScreen: "Full screen",
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
  openColleague: "Open",
  stillRunning: "Running commands",
  stopJobAsk: "Stop this command? The agent will be told you stopped it.",
  notStopped: "Couldn't stop it",
  cancel: "Cancel",
  jobRunning: (job, machine, lasted) => `${job} · ${machine} · ${lasted}`,
  minutes: (n) => (n < 1 ? "under a minute" : `${n} min`),
  hours: (hours, minutes) => (minutes ? `${hours} h ${minutes} min` : `${hours} h`),
  landed: (how, machine) =>
    how === "sent"
      ? `Sent to ${machine}; delivered when it arrives`
      : how === "uncertain"
        ? "Delivery unknown"
        : "Delivered at its next fastctx call",
  jobEnded: (job, machine) => `Job ${job} on ${machine} ended`,
  movedTo: (town) => `Moved to project ${town}`,
  movedOut: "Removed from project",
  briefHanded: "Project brief",
  deskFree: (who) => `${who}'s turn ended; Computer Use is free`,
  deskTaken: (who) => `${who} is using Computer Use; you'll be notified when it's free`,
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
