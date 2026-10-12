// What the agents page says (`world-gui.md`, 账号、机器、共享与借出): short
// labels on the slips, the change card's lines, and what stays a hover away.

import type { Installed } from "../world/logins";

export interface AgentLoginsWords {
  title: string;
  codex: string;
  claude: string;
  /** This computer, a machine of the user's, one shared by someone. */
  here: string;
  yours: string;
  sharedBy(owner: string): string;
  offline: string;
  noAnswer: string;
  /** Before the first scan of a machine. */
  scanHere: string;
  scanThere: string;
  rescan: string;
  scanning: string;
  /** The walk of every disk, its progress, and stopping it. */
  walking(directories: number): string;
  stopWalk: string;
  walkStopped: string;
  scanFailed: string;
  /** The programs found, at the head of the agent's column. */
  programs(n: number): string;
  noProgram: string;
  how: Record<Installed, string>;
  within(name: string, version: string | null): string;
  onPath: string;
  /** The groups of logins. */
  listed: string;
  candidates: string;
  elsewhere(n: number): string;
  noneFound: string;
  addDir: string;
  addDirPlaceholder: string;
  add: string;
  /** A slip. */
  defaultLogin: string;
  token: string;
  tokenSaid: string;
  loggedOut: string;
  applied: string;
  applying: string;
  removing: string;
  apply: string;
  more: string;
  remove: string;
  lend: string;
  rereadLimits: string;
  copyDir: string;
  copied: string;
  plan: string;
  planNone: string;
  lentTo: string;
  takeBack(account: string): string;
  /** The usage bars and their hover. */
  window(name: string, minutes: number | null): string;
  used(percent: string): string;
  resets(at: Date, now: Date): string;
  /** How long until a window starts again, as short as its name: 45m,
   *  2h20m, 3d4h; nothing once that time has passed. */
  resetsIn(at: Date, now: Date): string;
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
  scanHere: "扫描这台电脑",
  scanThere: "扫描这台机器",
  rescan: "重新扫描",
  scanning: "正在找",
  walking: (directories) => `正在走一遍整台电脑的盘 · 已看 ${directories.toLocaleString("zh-CN")} 个目录`,
  stopWalk: "停止",
  walkStopped: "整盘的查找已停止",
  scanFailed: "扫描没有完成",
  programs: (n) => `${n} 个程序`,
  noProgram: "没有找到它的程序",
  how,
  within: (name, version) => (version ? `${name} ${version} 自带` : `${name} 自带`),
  onPath: "终端里敲命令用的是它",
  listed: "已接入",
  candidates: "找到的登录",
  elsewhere: (n) => `别处还找到 ${n} 处`,
  noneFound: "没有找到登录",
  addDir: "添加目录",
  addDirPlaceholder: "登录目录的路径",
  add: "添加",
  defaultLogin: "默认",
  token: "长期令牌",
  tokenSaid: "用 setup-token 得到的长期令牌登录：认不出账号与套餐",
  loggedOut: "未登录",
  applied: "已接入",
  applying: "正在接入",
  removing: "正在移除",
  apply: "接入",
  more: "更多",
  remove: "移除",
  lend: "借给…",
  rereadLimits: "重读额度",
  copyDir: "复制目录",
  copied: "已复制",
  plan: "套餐",
  planNone: "不标",
  lentTo: "借给",
  takeBack: (account) => `收回 ${account}`,
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
  observed: (when) => `${when} 读到`,
  credits: (balance, unlimited) => (unlimited ? "超出后用 credits（不限）" : balance !== null ? `超出后用 credits，余 ${balance}` : "超出后用 credits"),
  creditsMark: "正在用 credits",
  notRead: (agent) => (agent === "claude" ? "还没读到：这个登录的会话干过一轮之后才有" : "还没读到"),
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
  scanHere: "Scan this computer",
  scanThere: "Scan this machine",
  rescan: "Scan again",
  scanning: "Looking",
  walking: (directories) => `Walking every disk · ${directories.toLocaleString("en")} directories seen`,
  stopWalk: "Stop",
  walkStopped: "The disk walk was stopped",
  scanFailed: "The scan did not finish",
  programs: (n) => (n === 1 ? "1 program" : `${n} programs`),
  noProgram: "None of its programs found",
  how: { ...how, official: "official installer", desktop_app: "desktop app", extension: "editor extension" },
  within: (name, version) => (version ? `inside ${name} ${version}` : `inside ${name}`),
  onPath: "What the command runs in a terminal",
  listed: "Hooked up",
  candidates: "Logins found",
  elsewhere: (n) => (n === 1 ? "1 more place found elsewhere" : `${n} more places found elsewhere`),
  noneFound: "No logins found",
  addDir: "Add a directory",
  addDirPlaceholder: "Path of the login directory",
  add: "Add",
  defaultLogin: "Default",
  token: "Token",
  tokenSaid: "Logged in with a long-lived setup-token: no account or plan to show",
  loggedOut: "Logged out",
  applied: "Hooked up",
  applying: "Hooking up",
  removing: "Removing",
  apply: "Hook up",
  more: "More",
  remove: "Remove",
  lend: "Lend to…",
  rereadLimits: "Read limits again",
  copyDir: "Copy directory",
  copied: "Copied",
  plan: "Plan",
  planNone: "None",
  lentTo: "Lent to",
  takeBack: (account) => `Take back from ${account}`,
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
  observed: (when) => `read ${when}`,
  credits: (balance, unlimited) => (unlimited ? "Then credits (unlimited)" : balance !== null ? `Then credits, ${balance} left` : "Then credits"),
  creditsMark: "On credits",
  notRead: (agent) => (agent === "claude" ? "Not read yet: a session in this login has to do one turn first" : "Not read yet"),
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
