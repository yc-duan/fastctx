// What the agents page says (`world-gui.md`, 账号、机器、共享与协助): short
// labels on the slips, the folders searched for logins, the change card's
// lines, and what stays a hover away.

import type { Installed } from "../world/logins";
import type { MachinePickWords } from "./agent-machines";
import type { FolderPickerWords } from "./folder-picker";
import { enFolderPicker, zhFolderPicker } from "./words";

export interface AgentLoginsWords extends MachinePickWords {
  title: string;
  codex: string;
  claude: string;
  /** This computer, a machine of the user's, one shared by someone. */
  here: string;
  yours: string;
  sharedBy(owner: string): string;
  offline: string;
  noAnswer: string;
  reading: string;
  /** A machine whose agents only assist the reader, with none of this
   *  agent's among them. */
  noneForYou(agent: "codex" | "claude"): string;
  /** Before the first scan of a machine. */
  scanHere: string;
  scanThere: string;
  rescan: string;
  scanning: string;
  scanFailed: string;
  /** The programs found, at the head of the agent's column. */
  programs(n: number): string;
  noProgram: string;
  how: Record<Installed, string>;
  within(name: string, version: string | null): string;
  /** A command-line program, as its tag on the hookup card names it. */
  commandLine(version: string | null): string;
  onPath: string;
  /** The groups of logins. */
  listed: string;
  candidates: string;
  noneFound: string;
  /** The folders searched for logins: the group, choosing one, a search
   *  under way and how it ended, and what stays a hover away. */
  folders: string;
  addFolder: string;
  searching(directories: number): string;
  stop: string;
  stopped: string;
  /** A listed login whose tools were not brought up to date by themselves:
   *  something of theirs changed since, or the output limit waits. */
  driftChanged: string;
  driftAsks: string;
  searchFailed: string;
  noneBelow: string;
  notSearched: string;
  searched(directories: number, unreadable: number, when: string): string;
  forget: string;
  moreFound(n: number): string;
  /** Choosing a folder on another machine, walked by its node. */
  picker: FolderPickerWords;
  /** A slip. */
  defaultLogin: string;
  token: string;
  tokenSaid: string;
  loggedOut: string;
  /** Claude Code older than the World starts conversations in, said
   *  before it is connected or used. */
  tooOld: string;
  tooOldSaid(version: string, oldest: string): string;
  applied: string;
  applying: string;
  removing: string;
  apply: string;
  more: string;
  remove: string;
  assist: string;
  rereadLimits: string;
  copyDir: string;
  copied: string;
  autoTitle: string;
  autoTitleWhy: string;
  plan: string;
  planNone: string;
  assisting: string;
  endAssist(account: string): string;
  /** A login assisting the reader: from whom. */
  assistedFrom(owner: string): string;
  weeklyShort(amount: string): string;
  dailyShort(amount: string): string;
  /** What a row says of this week, and its way to the login's page. */
  week: string;
  myWeek: string;
  openUsage: string;
  /** The usage bars and their hover. */
  window(name: string, minutes: number | null): string;
  used(percent: string): string;
  resets(at: Date, now: Date): string;
  /** How long until a window starts again, as short as its name: 45m,
   *  2h20m, 3d4h; nothing once that time has passed. */
  resetsIn(at: Date, now: Date): string;
  /** A window past the reset it was read before, where its time would be. */
  reset: string;
  /** What it had used when it was read, before that reset. */
  usedBefore(percent: string): string;
  observed(when: string): string;
  credits(balance: string | null, unlimited: boolean): string;
  creditsMark: string;
  notRead(agent: "codex" | "claude"): string;
  failed: string;
}

const how: Record<Installed, string> = {
  npm: "npm",
  pnpm: "pnpm",
  yarn: "Yarn",
  bun: "Bun",
  official: "官方安装",
  winget: "winget",
  scoop: "Scoop",
  chocolatey: "Chocolatey",
  homebrew: "Homebrew",
  desktop_app: "桌面 app",
  extension: "编辑器扩展",
  path: "PATH",
};

export const zhAgentLogins: AgentLoginsWords = {
  title: "智能体",
  codex: "Codex",
  claude: "Claude Code",
  here: "本机",
  yours: "你的",
  sharedBy: (owner) => `${owner} 共享`,
  offline: "离线",
  noAnswer: "没有回应",
  reading: "正在读",
  noneForYou: (agent) => `这台机器上的 ${agent === "codex" ? "Codex" : "Claude Code"} 没有协助你`,
  holds: { mine: "我的机器", shared: "共享给我的", others: "其他成员的", assist: "协助我的" },
  assistAgents: (owner) => `${owner} 的智能体`,
  ownedBy: (owner) => `${owner} 的`,
  pickMachine: "换一台机器",
  scanHere: "扫描这台电脑",
  scanThere: "扫描这台机器",
  rescan: "重新扫描",
  scanning: "正在找",
  scanFailed: "扫描没有完成",
  programs: (n) => `${n} 个程序`,
  noProgram: "没有找到它的程序",
  how,
  within: (name, version) => (version ? `${name} ${version} 自带` : `${name} 自带`),
  commandLine: (version) => (version ? `命令行 ${version}` : "命令行"),
  onPath: "终端里敲命令用的是它",
  listed: "已接入",
  candidates: "找到的登录",
  noneFound: "没有找到登录",
  folders: "文件夹",
  addFolder: "添加文件夹…",
  searching: (directories) => `已看 ${directories.toLocaleString("zh-CN")} 个文件夹`,
  stop: "停止",
  stopped: "已停止",
  driftChanged: "接入被改过",
  driftAsks: "接入要更新",
  searchFailed: "没找完",
  noneBelow: "没找到",
  notSearched: "还没找过",
  searched: (directories, unreadable, when) =>
    [`看了 ${directories.toLocaleString("zh-CN")} 个文件夹`, unreadable ? `${unreadable.toLocaleString("zh-CN")} 个打不开` : "", when].filter(Boolean).join("，"),
  forget: "不再找这个文件夹",
  moreFound: (n) => `还有 ${n} 处`,
  picker: zhFolderPicker,
  defaultLogin: "默认",
  token: "长期令牌",
  tokenSaid: "用 setup-token 得到的长期令牌登录：认不出账号与套餐",
  loggedOut: "未登录",
  tooOld: "版本太旧",
  tooOldSaid: (version, oldest) => `这里的 Claude Code 是 ${version}，要升级到 ${oldest} 或更新`,
  applied: "已接入",
  applying: "正在接入",
  removing: "正在移除",
  apply: "接入",
  more: "更多",
  remove: "移除",
  assist: "协助…",
  rereadLimits: "重读额度",
  copyDir: "复制目录",
  copied: "已复制",
  autoTitle: "自动命名对话",
  autoTitleWhy: "新对话的第一轮结束后，由这个登录起一个标题，和 Codex 官方客户端一样；每段对话多一次很小的模型请求",
  plan: "套餐",
  planNone: "不标",
  assisting: "协助",
  endAssist: (account) => `结束对 ${account} 的协助`,
  assistedFrom: (owner) => `来自 ${owner}`,
  weeklyShort: (amount) => `${amount}/周`,
  dailyShort: (amount) => `${amount}/天`,
  week: "本周",
  myWeek: "我本周",
  openUsage: "看用量",
  window: (name, minutes) => {
    if (name.startsWith("five_hour") || minutes === 300) return "5h";
    if (name.startsWith("seven_day") || minutes === 10080) return "7d";
    if (name.startsWith("spend_limit")) return "花费";
    if (minutes !== null) return minutes % 1440 === 0 ? `${minutes / 1440}d` : minutes % 60 === 0 ? `${minutes / 60}h` : `${minutes}m`;
    return name;
  },
  used: (percent) => `已用 ${percent}`,
  resets: (at, now) => {
    const minutes = Math.max(0, Math.round((at.getTime() - now.getTime()) / 60000));
    if (minutes < 60) return `${minutes} 分钟后重置`;
    if (minutes < 1440) return `${Math.round(minutes / 60)} 小时后重置`;
    return `${["周日", "周一", "周二", "周三", "周四", "周五", "周六"][at.getDay()]} ${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")} 重置`;
  },
  resetsIn: (at, now) => left(at, now),
  reset: "已重置",
  usedBefore: (percent) => `重置前已用 ${percent}`,
  observed: (when) => `${when} 读到`,
  credits: (balance, unlimited) => (unlimited ? "超出后用 credits（不限）" : balance !== null ? `超出后用 credits，余 ${balance}` : "超出后用 credits"),
  creditsMark: "正在用 credits",
  notRead: (agent) => (agent === "claude" ? "还没读到：这个登录的会话干过一轮之后才有" : "还没读到：这个登录的对话在 Fastctx 里干过一轮之后才有"),
  failed: "没办成",
};

export const enAgentLogins: AgentLoginsWords = {
  title: "Agents",
  codex: "Codex",
  claude: "Claude Code",
  here: "This computer",
  yours: "Yours",
  sharedBy: (owner) => `Shared by ${owner}`,
  offline: "Offline",
  noAnswer: "No answer",
  reading: "Reading",
  noneForYou: (agent) => `No ${agent === "codex" ? "Codex" : "Claude Code"} login here assists you`,
  holds: { mine: "My machines", shared: "Shared with me", others: "Other members'", assist: "Assisting me" },
  assistAgents: (owner) => `${owner}'s agents`,
  ownedBy: (owner) => `${owner}'s`,
  pickMachine: "Choose a machine",
  scanHere: "Scan this computer",
  scanThere: "Scan this machine",
  rescan: "Scan again",
  scanning: "Looking",
  scanFailed: "The scan did not finish",
  programs: (n) => (n === 1 ? "1 program" : `${n} programs`),
  noProgram: "None of its programs found",
  how: { ...how, official: "official installer", desktop_app: "desktop app", extension: "editor extension" },
  within: (name, version) => (version ? `inside ${name} ${version}` : `inside ${name}`),
  commandLine: (version) => (version ? `CLI ${version}` : "CLI"),
  onPath: "What the command runs in a terminal",
  listed: "Hooked up",
  candidates: "Logins found",
  noneFound: "No logins found",
  folders: "Folders",
  addFolder: "Add a folder…",
  searching: (directories) => `${directories.toLocaleString("en")} folders seen`,
  stop: "Stop",
  stopped: "Stopped",
  driftChanged: "Changed since",
  driftAsks: "Needs update",
  searchFailed: "Not finished",
  noneBelow: "None found",
  notSearched: "Not searched yet",
  searched: (directories, unreadable, when) =>
    [`${directories.toLocaleString("en")} folders seen`, unreadable ? `${unreadable.toLocaleString("en")} unreadable` : "", when].filter(Boolean).join(", "),
  forget: "Stop searching this folder",
  moreFound: (n) => `${n} more`,
  picker: enFolderPicker,
  defaultLogin: "Default",
  token: "Token",
  tokenSaid: "Logged in with a long-lived setup-token: no account or plan to show",
  loggedOut: "Logged out",
  tooOld: "Too old",
  tooOldSaid: (version, oldest) => `Claude Code here is ${version}; update it to ${oldest} or later`,
  applied: "Hooked up",
  applying: "Hooking up",
  removing: "Removing",
  apply: "Hook up",
  more: "More",
  remove: "Remove",
  assist: "Assist…",
  rereadLimits: "Read limits again",
  copyDir: "Copy directory",
  copied: "Copied",
  autoTitle: "Name conversations",
  autoTitleWhy: "After a new conversation's first turn, this login names it, as the Codex app does: one small model request per conversation",
  plan: "Plan",
  planNone: "None",
  assisting: "Assisting",
  endAssist: (account) => `Stop assisting ${account}`,
  assistedFrom: (owner) => `From ${owner}`,
  weeklyShort: (amount) => `${amount}/wk`,
  dailyShort: (amount) => `${amount}/day`,
  week: "This week",
  myWeek: "My week",
  openUsage: "Usage",
  window: (name, minutes) => {
    if (name.startsWith("five_hour") || minutes === 300) return "5h";
    if (name.startsWith("seven_day") || minutes === 10080) return "7d";
    if (name.startsWith("spend_limit")) return "spend";
    if (minutes !== null) return minutes % 1440 === 0 ? `${minutes / 1440}d` : minutes % 60 === 0 ? `${minutes / 60}h` : `${minutes}m`;
    return name;
  },
  used: (percent) => `${percent} used`,
  resets: (at, now) => {
    const minutes = Math.max(0, Math.round((at.getTime() - now.getTime()) / 60000));
    if (minutes < 60) return `resets in ${minutes} min`;
    if (minutes < 1440) return `resets in ${Math.round(minutes / 60)} h`;
    return `resets ${at.toLocaleString("en", { weekday: "short", hour: "2-digit", minute: "2-digit" })}`;
  },
  resetsIn: (at, now) => left(at, now),
  reset: "reset",
  usedBefore: (percent) => `${percent} used before it reset`,
  observed: (when) => `read ${when}`,
  credits: (balance, unlimited) => (unlimited ? "Then credits (unlimited)" : balance !== null ? `Then credits, ${balance} left` : "Then credits"),
  creditsMark: "On credits",
  notRead: (agent) => (agent === "claude" ? "Not read yet: a session in this login has to do one turn first" : "Not read yet: a conversation in this login has to do one turn in Fastctx first"),
  failed: "Not done",
};

/** Time left, in the units the windows are named in, the two largest. */
function left(at: Date, now: Date): string {
  const minutes = Math.ceil((at.getTime() - now.getTime()) / 60_000);
  if (minutes <= 0) return "";
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 1440) return minutes % 60 ? `${Math.floor(minutes / 60)}h${minutes % 60}m` : `${minutes / 60}h`;
  const hours = Math.floor((minutes % 1440) / 60);
  return hours ? `${Math.floor(minutes / 1440)}d${hours}h` : `${Math.floor(minutes / 1440)}d`;
}
