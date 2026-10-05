import type { QueueWords } from "./conversation/queue";
// Every word the interface says on its own account, in the user's words
// (`world-gui.md`, 用词): plain, without role-play; what agents and tools
// produce is never translated.

import type { FindWords } from "./conversation/find";

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
  queue: QueueWords;
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
  idle: string;
  waitingForYou: string;
  on(machine: string): string;
  /** Between the names in a short list. */
  listSeparator: string;
  moreLines(n: number): string;
  unchangedLines(n: number): string;
  /** The watch on the session ended: the node ended it, or the connection
   *  to it broke. */
  watchEnded(why: string | null): string;
  /** Words that could not be sent. */
  notSent(why: string): string;
  /** The way into a colleague's own conversation. */
  openColleague: string;
  /** What it started that still runs. */
  stillRunning: string;
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
  /** How a job or a colleague's turn ended, from the World's own wording
   *  (`exited 0`, `turn completed, 14m`); a wording not known is kept. */
  ended(how: string): string;
}

const zhActs: Record<Act, [string, string, string]> = {
  read: ["读了", "正在读", "读取失败"],
  edit: ["改了", "正在改", "修改失败"],
  create: ["新建了", "正在新建", "新建失败"],
  delete: ["删了", "正在删", "删除失败"],
  move: ["挪了", "正在挪", "挪动失败"],
  write: ["写了", "正在写", "写入失败"],
  command: ["跑了命令", "正在跑命令", "命令失败"],
  search: ["搜了", "正在搜", "搜索失败"],
  image: ["看了图", "正在看图", "看图失败"],
  ask: ["请了同事", "正在请同事", "请同事失败"],
  message: ["给同事发了话", "正在给同事发话", "发话失败"],
  fetch: ["取了网页", "正在取网页", "取网页失败"],
  websearch: ["搜了网页", "正在搜网页", "网页搜索失败"],
  tasks: ["更新了待办", "正在更新待办", "待办更新失败"],
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
  queue: {
    waiting: (count) => `排着的 ${count} 句，这一轮说完后一句一句发出`,
    held: "这一轮停下了，排着的先留着；点「现在发」接着发",
    notTaken: "上一句没发出去（见对话里），排着的先留着；点「现在发」接着发",
    unsure: "上一句送没送进它这一轮说不准（见对话里），排着的先留着；点「现在发」接着发",
    sendNow: "现在发",
    sendNowTitle: "不等这一轮说完，现在就发给它",
    edit: "改",
    save: "保存",
    cancel: "算了",
    remove: "拿掉",
    reorder: "按住拖动换顺序；也可以 Alt 加上下方向键",
    files: (count) => `${count} 个文件`,
    empty: "没有要发的",
    queue: "排到这一轮之后",
  },
  act: (act, tense) => zhActs[act][index(tense)],
  failedTool: (name) => `${name} 失败`,
  tally: (t) => {
    const parts: string[] = [];
    if (t.read) parts.push(`读了 ${t.read} 个文件`);
    if (t.edited) parts.push(`改了 ${t.edited} 个文件`);
    if (t.ran) parts.push(`跑了 ${t.ran} 条命令`);
    if (t.searched) parts.push(`搜了 ${t.searched} 次`);
    if (t.asked) parts.push(`请了 ${t.asked} 位同事`);
    if (t.other) parts.push(`用了 ${t.other} 个工具`);
    let heading = parts.join("，");
    if (!heading) heading = t.thinking ? "思考" : "活动";
    return t.failed ? `${heading} · ${t.failed} 个失败` : heading;
  },
  thought: "思考",
  thinking: "正在思考",
  thoughtWithheld: "想过，没有交出内容",
  plan: "提出的计划",
  copyMessage: "复制这段话",
  copyCode: "复制代码",
  rewind: "改了重发",
  fork: "从这里分叉",
  forkedFrom: (source) => `从 ${source} 的对话分出来`,
  find: {
    find: "在这段对话里找",
    placeholder: "在这段对话里找…",
    count: (current, total) => `${current} / ${total}`,
    none: "没有找到",
    previous: "上一个",
    next: "下一个",
    close: "关闭查找",
  },
  you: (machine) => `你 · ${machine}`,
  world: "世界",
  compacted: "上下文已压缩",
  turn: (how, reason) => {
    if (how === "completed") return null;
    const said = how === "failed" ? "这一轮失败了" : how === "interrupted" ? "这一轮被停下了" : `这一轮结束：${how}`;
    return reason ? `${said}：${reason}` : said;
  },
  wentOnAs: (job) => `转到后台 ${job}`,
  exited: (code) => `退出码 ${code}`,
  cut: "这一轮结束时还没跑完",
  unknown: "结果不明",
  declined: "没让它用",
  showMore: "展开全部",
  showLess: "收起",
  copy: "复制",
  copied: "已复制",
  sendPlaceholder: (agent) => `对 ${agent} 说…`,
  send: "发送",
  stop: "停下这一轮",
  uncertain: (why) => `话交出去了，但不知道它进没进去：${why}`,
  sending: "正在送出",
  context: (percent) => `上下文 ${percent}%`,
  working: "正在干活",
  idle: "空闲",
  waitingForYou: "在等你回答",
  on: (machine) => `在 ${machine}`,
  listSeparator: "、",
  moreLines: (n) => `还有 ${n} 行`,
  unchangedLines: (n) => `${n} 行未改动`,
  watchEnded: (why) => (why ? `没在看着它了：${why}` : "没在看着它了"),
  notSent: (why) => `没送出去：${why}`,
  openColleague: "看它",
  stillRunning: "它起的、还在跑的",
  jobRunning: (job, machine, lasted) => `${job} 在 ${machine} 上跑了 ${lasted}`,
  minutes: (n) => (n < 1 ? "不到一分钟" : `${n} 分钟`),
  hours: (hours, minutes) => (minutes ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`),
  landed: (how, machine, why) =>
    how === "sent"
      ? `寄到了 ${machine}，那台机器收到时送进去`
      : how === "uncertain"
        ? `话交出去了，但不知道它进没进去${why ? `：${why}` : ""}`
        : `它此刻没给世界开口，话等它下次用世界的工具时送到${why ? `（${why}）` : ""}`,
  jobEnded: (job, machine) => `后台作业 ${job} 在 ${machine} 上结束了`,
  ended: (how) =>
    endedIn(how, {
      exited: (code) => `退出码 ${code}`,
      signal: (signal) => `被信号 ${signal} 结束`,
      stopped: "被停掉了",
      finished: "跑完了",
      interrupted: (reason) => `中断：${reason}`,
      couldNotRun: (error) => `没能跑起来：${error}`,
      turn: (how, reason) =>
        (how === "completed" ? "这一轮完成" : how === "failed" ? "这一轮失败" : how === "interrupted" ? "这一轮中断" : `这一轮 ${how}`) +
        (reason ? `：${reason}` : ""),
      inTurn: "这一轮还在跑",
    }),
};

export const en: Words = {
  queue: {
    waiting: (count) => `${count} queued; sent one by one once this turn ends`,
    held: "This turn was stopped; what is queued waits. Send one to go on",
    notTaken: "The last one was not sent (see above); the rest wait. Send one to go on",
    unsure: "Whether the last one reached its turn is not known (see above); the rest wait. Send one to go on",
    sendNow: "Send now",
    sendNowTitle: "Send it now, without waiting for this turn to end",
    edit: "Edit",
    save: "Save",
    cancel: "Cancel",
    remove: "Remove",
    reorder: "Hold and drag to reorder, or Alt with the up and down arrows",
    files: (count) => `${count} files`,
    empty: "Nothing to send",
    queue: "Queue after this turn",
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
  thoughtWithheld: "Thought, without sharing it",
  plan: "Proposed plan",
  copyMessage: "Copy this message",
  copyCode: "Copy code",
  rewind: "Edit and resend",
  fork: "Fork from here",
  forkedFrom: (source) => `Forked from ${source}`,
  find: {
    find: "Find in this conversation",
    placeholder: "Find in this conversation…",
    count: (current, total) => `${current} of ${total}`,
    none: "No matches",
    previous: "Previous",
    next: "Next",
    close: "Close find",
  },
  you: (machine) => `You · ${machine}`,
  world: "The World",
  compacted: "Context compacted",
  turn: (how, reason) => {
    if (how === "completed") return null;
    const said = how === "failed" ? "The turn failed" : how === "interrupted" ? "The turn was stopped" : `The turn ended: ${how}`;
    return reason ? `${said}: ${reason}` : said;
  },
  wentOnAs: (job) => `went on as ${job}`,
  exited: (code) => `exit ${code}`,
  cut: "still running when the turn ended",
  unknown: "outcome unknown",
  declined: "not allowed",
  showMore: "Show more",
  showLess: "Show less",
  copy: "Copy",
  copied: "Copied",
  sendPlaceholder: (agent) => `Say something to ${agent}…`,
  send: "Send",
  stop: "Stop the turn",
  uncertain: (why) => `Sent, but whether it went in is unknown: ${why}`,
  sending: "Sending",
  context: (percent) => `Context ${percent}%`,
  working: "Working",
  idle: "Idle",
  waitingForYou: "Waiting for you",
  on: (machine) => `on ${machine}`,
  listSeparator: ", ",
  moreLines: (n) => `${n} more lines`,
  unchangedLines: (n) => `${n} unchanged lines`,
  watchEnded: (why) => (why ? `No longer watching: ${why}` : "No longer watching"),
  notSent: (why) => `Not sent: ${why}`,
  openColleague: "Open",
  stillRunning: "Still running, started here",
  jobRunning: (job, machine, lasted) => `${job} on ${machine}, running ${lasted}`,
  minutes: (n) => (n < 1 ? "under a minute" : `${n} min`),
  hours: (hours, minutes) => (minutes ? `${hours} h ${minutes} min` : `${hours} h`),
  landed: (how, machine, why) =>
    how === "sent"
      ? `Sent to ${machine}; it goes in when that machine has it`
      : how === "uncertain"
        ? `Handed over, but whether it went in is unknown${why ? `: ${why}` : ""}`
        : `It gives the World no way in now; the words wait for its next call of a World tool${why ? ` (${why})` : ""}`,
  jobEnded: (job, machine) => `Job ${job} on ${machine} ended`,
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
