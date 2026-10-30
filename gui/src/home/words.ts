// What the World's front page says, in the user's words (`world-gui.md`,
// 用词): agents by who they are and where, projects and machines by name,
// times as people say them. Nothing here is a judgment; every line says a
// fact the World holds. A project's page says some of the same.

import type { Asked } from "../world/world";

export interface HomeWords {
  /** Where agents assisting the user run, said as whose agents. */
  assistAgents(owner: string): string;
  /** An agent assisting the user, as whose it is, never its machine. */
  someoneAssisting(agent: string, owner: string): string;
  world: string;
  /** An open page, as who started or wrote something. */
  aPage(name: string): string;
  /** Conversations waiting for the reader. */
  waiting: string;
  /** What an urgent one is called where its mark is not seen. */
  urgent: string;
  /** What an agent asks, said whole to a screen reader. */
  asks: Record<Asked["what"], string>;
  /** A conversation the host says waits on the reader, saying nothing more. */
  waitsUnsaid: string;
  /** What a conversation that waits on the reader does, as a project's page
   *  says it. */
  waitingFor: string;
  /** An answer given here, before its conversation takes it. */
  /** Conversations at work, and the commands running. */
  now: string;
  jobs: string;
  /** Stopping a running command, asked once more before it is done. */
  stopJob: string;
  stopJobAsk: string;
  cancel: string;
  jobNotStopped: string;
  nobodyWorking: string;
  /** Begins a conversation, where nobody is at work. */
  begin: string;
  /** What came back while the reader was away. */
  away: string;
  nothingAway: string;
  /** The page's two views, and what now says when nothing needs the user. */
  nowView: string;
  latelyView: string;
  nothingNeeds: string;
  /** The end of the line of the time away. */
  present: string;
  /** A turn that ended in an error, and why. */
  failed(reason: string): string;
  /** A turn that compacted the conversation and said nothing else. */
  compacted: string;
  /** Between the names in a short list. */
  listSeparator: string;
  /** The World could not be asked, and why. */
  unreachable(why: string): string;
  /** The World here is older than the interface and refuses it. */
  oldWorld(node: string, client: string): string;
  /** Where the new one is put in place. */
  renewThere: string;
  /** The way to set this computer up, beside the World it could not ask. */
  setUp: string;
  /** "laptop 上的 Claude": an agent by its program and machine. */
  someone(agent: string, machine: string): string;
  colleagueOf(asker: string): string;
  for(duration: string): string;
  ago(ms: number): string;
  lasted(ms: number): string;
  clock(at: Date): string;
  day(at: Date): string;
  /** A moment the day before, or earlier, said short: 昨天 22:50, 10月1日 22:50. */
  earlier(at: Date, yesterday: boolean): string;
  you: string;
  unasked(list: string): string;
  awayTrouble(why: string): string;
  startedBy(who: string): string;
  /** Said of a job or agent that works for other accounts than the reader's. */
  forAccounts(accounts: string): string;
}

const zhWeek = ["星期日", "星期一", "星期二", "星期三", "星期四", "星期五", "星期六"];
const zhDigits = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];

/** 1 to 31 in Chinese numerals, as a date is written: 四、十、十二、二十一. */
function zhNumber(n: number): string {
  if (n < 10) return zhDigits[n];
  const tens = Math.floor(n / 10);
  const ones = n % 10;
  return `${tens === 1 ? "" : zhDigits[tens]}十${ones ? zhDigits[ones] : ""}`;
}

function zhSpan(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "不到一分钟";
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 && hours < 6 ? `${hours} 小时 ${minutes % 60} 分` : `${hours} 小时`;
  return `${Math.floor(hours / 24)} 天`;
}

function enSpan(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 && hours < 6 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
  const days = Math.floor(hours / 24);
  return `${days} day${days === 1 ? "" : "s"}`;
}

const pad = (n: number) => String(n).padStart(2, "0");

export const zhHome: HomeWords = {
  world: "世界",
  aPage: (name) => `页面「${name}」`,
  waiting: "要紧的事",
  urgent: "要紧",
  asks: { answer: "问你", command: "想运行命令", change: "想改文件", tool: "想用工具", plan: "等你批准计划", form: "请你填写", other: "等你回复" },
  waitsUnsaid: "等你回复",
  waitingFor: "等待回复",
  now: "工作中",
  jobs: "后台命令",
  stopJob: "停止",
  stopJobAsk: "停下这条命令？",
  cancel: "取消",
  jobNotStopped: "没能停下",
  nobodyWorking: "暂无工作中的智能体",
  begin: "新对话",
  away: "离开期间",
  nothingAway: "没有新动态",
  nowView: "此刻",
  latelyView: "近况",
  nothingNeeds: "没有在等你的事",
  present: "现在",
  failed: (reason) => `出错：${reason}`,
  compacted: "对话已压缩",
  listSeparator: "、",
  unreachable: () => "无法连接本机服务",
  oldWorld: (node, client) => `本机服务版本 ${node} 与界面 ${client} 不兼容`,
  renewThere: "更新本机服务",
  setUp: "设置这台电脑",
  someone: (agent, machine) => `${machine} 上的 ${agent}`,
  someoneAssisting: (agent, owner) => `${owner} 的 ${agent}`,
  assistAgents: (owner) => `${owner} 的智能体`,
  colleagueOf: (asker) => `${asker} 调用的模型`,
  for: (duration) => duration,
  ago: (ms) => `${zhSpan(ms)}前`,
  lasted: zhSpan,
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
  day: (at) => `${zhNumber(at.getMonth() + 1)}月${zhNumber(at.getDate())}日　${zhWeek[at.getDay()]}`,
  earlier: (at, yesterday) => `${yesterday ? "昨天" : `${at.getMonth() + 1}月${at.getDate()}日`} ${pad(at.getHours())}:${pad(at.getMinutes())}`,
  you: "你",
  unasked: (list) => `未获取：${list}`,
  awayTrouble: () => "无法获取离开期间的动态",
  startedBy: (who) => `由 ${who} 启动`,
  forAccounts: (accounts) => `属于 ${accounts}`,
};

const enWeek = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const enMonths = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

export const enHome: HomeWords = {
  world: "Home",
  aPage: (name) => `the page “${name}”`,
  waiting: "Needs you",
  urgent: "Urgent",
  asks: {
    answer: "asks you",
    command: "wants to run a command",
    change: "wants to change files",
    tool: "wants to use a tool",
    plan: "waits for its plan's approval",
    form: "asks you to fill in a form",
    other: "waits for you",
  },
  waitsUnsaid: "Waiting for your reply",
  waitingFor: "Waiting for your reply",
  now: "Working",
  jobs: "Background commands",
  stopJob: "Stop",
  stopJobAsk: "Stop this command?",
  cancel: "Cancel",
  jobNotStopped: "Couldn't stop",
  nobodyWorking: "No agents working",
  begin: "New conversation",
  away: "While you were away",
  nowView: "Now",
  latelyView: "Lately",
  nothingNeeds: "Nothing needs you",
  nothingAway: "Nothing new",
  present: "now",
  failed: (reason) => `Error: ${reason}`,
  compacted: "Conversation compacted",
  listSeparator: ", ",
  unreachable: () => "Can't connect to the local service",
  oldWorld: (node, client) => `Local service ${node} isn't compatible with this app (${client})`,
  renewThere: "Update this computer's service",
  setUp: "Set up this computer",
  someone: (agent, machine) => `${agent} on ${machine}`,
  someoneAssisting: (agent, owner) => `${owner}'s ${agent}`,
  assistAgents: (owner) => `${owner}'s agents`,
  colleagueOf: (asker) => `called by ${asker}`,
  for: (duration) => duration,
  ago: (ms) => `${enSpan(ms)} ago`,
  lasted: enSpan,
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
  day: (at) => `${enWeek[at.getDay()]}, ${enMonths[at.getMonth()]} ${at.getDate()}`,
  earlier: (at, yesterday) => `${yesterday ? "yesterday" : `${enMonths[at.getMonth()].slice(0, 3)} ${at.getDate()}`} ${pad(at.getHours())}:${pad(at.getMinutes())}`,
  you: "You",
  unasked: (list) => `Not asked: ${list}`,
  awayTrouble: () => "Couldn't load activity while you were away",
  startedBy: (who) => `started by ${who}`,
  forAccounts: (accounts) => `belongs to ${accounts}`,
};
