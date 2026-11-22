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
  /** A listed login whose tools could not be brought up to date by
   *  themselves; an older Fastctx's that waits for the output limit. */
  driftChanged: string;
  driftAsks: string;
  /** Putting the tools in again as this version writes them. */
  renew: string;
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
  /** Opening the workbench in a connected login, and closing it. */
  bench: string;
  benchSaid: string;
  assist: string;
  rereadLimits: string;
  copyDir: string;
  copied: string;
  autoTitle: string;
  autoTitleWhy: string;
  resetInPlace: string;
  resetInPlaceWhy: string;
  /** A Claude Code login's models, as those who hold it change them. */
  models: string;
  modelsTitle: string;
  modelsOf(who: string): string;
  modelsWhat: string;
  modelsOwn: string;
  modelsRemove: string;
  modelsLast: string;
  modelsAdd: string;
  modelsAddHint: string;
  modelsAddGo: string;
  modelsAddWhy: string;
  modelsRemoved: string;
  modelsBack: string;
  modelsReset: string;
  modelsDone: string;
  modelsClose: string;
  modelsFailed(why: string): string;
  /** Compacting a conversation idle a while, as the reader sets it for one
   *  login, apart from everyone else who uses it. */
  idle: string;
  idleTitle: string;
  idleOf(who: string): string;
  idleOn: string;
  idleOnSaid: string;
  idleAfter: string;
  idleAfterSaid: string;
  idleMinutes: string;
  idleMin: string;
  idleMinSaid: string;
  /** The unit the least context is written in, and how many tokens it is. */
  idleTokens: string;
  idleTokensEach: number;
  idleRange(least: number, most: number): string;
  idleFailed(why: string): string;
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
  noAnswer: "无应答",
  reading: "正在读取",
  noneForYou: (agent) => `该机器上的 ${agent === "codex" ? "Codex" : "Claude Code"} 未协助你`,
  holds: { mine: "我的机器", shared: "共享给我的", others: "其他成员的", assist: "协助我的" },
  assistAgents: (owner) => `${owner} 的智能体`,
  ownedBy: (owner) => `${owner} 的`,
  pickMachine: "切换机器",
  scanHere: "扫描本机",
  scanThere: "扫描该机器",
  rescan: "重新扫描",
  scanning: "正在扫描",
  scanFailed: "扫描未完成",
  programs: (n) => `${n} 个程序`,
  noProgram: "未找到程序",
  how,
  within: (name, version) => (version ? `${name} ${version} 自带` : `${name} 自带`),
  commandLine: (version) => (version ? `命令行 ${version}` : "命令行"),
  onPath: "终端命令使用此程序",
  listed: "已接入",
  candidates: "发现的登录",
  noneFound: "未找到登录",
  folders: "文件夹",
  addFolder: "添加文件夹…",
  searching: (directories) => `已查找 ${directories.toLocaleString("zh-CN")} 个文件夹`,
  stop: "停止",
  stopped: "已停止",
  driftChanged: "接入已被改动",
  driftAsks: "接入待更新",
  renew: "更新接入",
  searchFailed: "未完成",
  noneBelow: "未找到",
  notSearched: "尚未查找",
  searched: (directories, unreadable, when) =>
    [`已查找 ${directories.toLocaleString("zh-CN")} 个文件夹`, unreadable ? `${unreadable.toLocaleString("zh-CN")} 个无法读取` : "", when].filter(Boolean).join("，"),
  forget: "不再查找此文件夹",
  moreFound: (n) => `另有 ${n} 处`,
  picker: zhFolderPicker,
  defaultLogin: "默认",
  token: "长期令牌",
  tokenSaid: "以 setup-token 获得的长期令牌登录，无法识别账号与套餐",
  loggedOut: "未登录",
  tooOld: "版本过旧",
  tooOldSaid: (version, oldest) => `此处 Claude Code 版本为 ${version}，需升级到 ${oldest} 或更高`,
  applied: "已接入",
  applying: "正在接入",
  removing: "正在移除",
  apply: "接入",
  more: "更多",
  remove: "移除",
  bench: "工作台",
  benchSaid: "在 Fastctx 中对话；开启或关闭前先列出将做的改动",
  assist: "协助…",
  rereadLimits: "重读额度",
  copyDir: "复制目录",
  copied: "已复制",
  autoTitle: "自动命名对话",
  autoTitleWhy: "新对话发出首条消息时，由该登录生成标题，与 Codex 官方客户端一致；每个对话多一次小型模型请求",
  resetInPlace: "自动检测重置",
  resetInPlaceWhy: "周剩余额度恢复到98%—100%且增加至少2个百分点，经至少5分钟变化的新读数确认后重算本周；关闭不撤销已确认的重置",
  models: "模型…",
  modelsTitle: "模型",
  modelsOf: (who) => `${who} 的模型`,
  modelsWhat: "使用该登录的所有人（含受协助者）新建对话或切换模型时，从此清单中选择。",
  modelsOwn: "自定义",
  modelsRemove: "不再提供",
  modelsLast: "至少留一个",
  modelsAdd: "添加模型",
  modelsAddHint: "完整模型 ID，如 claude-opus-5-6",
  modelsAddGo: "添加",
  modelsAddWhy: "填写 Claude Code --model 可识别的完整 ID。Fastctx 无法得知其推理强度档位，使用时不单独选择强度。",
  modelsRemoved: "已移除的自带模型",
  modelsBack: "加回",
  modelsReset: "恢复 Fastctx 默认清单",
  modelsDone: "完成",
  modelsClose: "关闭",
  modelsFailed: (why) => `修改失败：${why}`,
  idle: "闲置自动压缩…",
  idleTitle: "闲置自动压缩",
  idleOf: (who) => `${who} 的闲置自动压缩`,
  idleOn: "闲置自动压缩",
  idleOnSaid: "对话闲置够久后，自动执行一次 /compact。只对你自己的对话生效。",
  idleAfter: "闲置多久后",
  idleAfterSaid: "Claude Code 订阅登录缓存约 1 小时，宜在过期前压缩；API key 登录只缓存 5 分钟，不会触发。",
  idleMinutes: "分钟",
  idleMin: "上下文至少",
  idleMinSaid: "上下文小于这个数的对话不压。",
  idleTokens: "万 token",
  idleTokensEach: 10_000,
  idleRange: (least, most) => `填 ${least} 到 ${most} 之间的数`,
  idleFailed: (why) => `保存失败：${why}`,
  plan: "套餐",
  planNone: "不标注",
  assisting: "协助",
  endAssist: (account) => `结束对 ${account} 的协助`,
  assistedFrom: (owner) => `来自 ${owner}`,
  weeklyShort: (amount) => `${amount}/周`,
  dailyShort: (amount) => `${amount}/天`,
  week: "本周",
  myWeek: "我本周",
  openUsage: "查看用量",
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
  notRead: (agent) => (agent === "claude" ? "尚未读取：该登录的会话完成一轮后可读取" : "尚未读取：该登录的对话在 Fastctx 中完成一轮后可读取"),
  failed: "操作失败",
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
  scanning: "Scanning",
  scanFailed: "The scan did not finish",
  programs: (n) => (n === 1 ? "1 program" : `${n} programs`),
  noProgram: "No program found",
  how: { ...how, official: "official installer", desktop_app: "desktop app", extension: "editor extension" },
  within: (name, version) => (version ? `inside ${name} ${version}` : `inside ${name}`),
  commandLine: (version) => (version ? `CLI ${version}` : "CLI"),
  onPath: "What the command runs in a terminal",
  listed: "Connected",
  candidates: "Logins found",
  noneFound: "No logins found",
  folders: "Folders",
  addFolder: "Add a folder…",
  searching: (directories) => `${directories.toLocaleString("en")} folders seen`,
  stop: "Stop",
  stopped: "Stopped",
  driftChanged: "Not up to date",
  driftAsks: "Needs update",
  renew: "Update connection",
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
  applied: "Connected",
  applying: "Connecting",
  removing: "Removing",
  apply: "Connect",
  more: "More",
  remove: "Remove",
  bench: "Workbench",
  benchSaid: "Talk to it in Fastctx; what opening or closing it changes is shown first",
  assist: "Assist…",
  rereadLimits: "Read limits again",
  copyDir: "Copy directory",
  copied: "Copied",
  autoTitle: "Name conversations",
  autoTitleWhy: "As a new conversation's first words go in, this login names it, as the Codex app does: one small model request per conversation",
  resetInPlace: "Detect resets automatically",
  resetInPlaceWhy: "Detect a weekly recovery to 98–100% remaining, rising by at least 2 points, confirmed by changed readings over at least 5 minutes. Turning this off keeps confirmed resets",
  models: "Models…",
  modelsTitle: "Models",
  modelsOf: (who) => `Models of ${who}`,
  modelsWhat: "Everyone who uses this login, those it assists too, picks from this list when starting a conversation or switching models.",
  modelsOwn: "Custom",
  modelsRemove: "Stop offering",
  modelsLast: "Keep at least one",
  modelsAdd: "Add a model",
  modelsAddHint: "A full model ID, such as claude-opus-5-6",
  modelsAddGo: "Add",
  modelsAddWhy: "A full ID Claude Code's --model takes. Fastctx does not know which efforts it has, so none is chosen with it.",
  modelsRemoved: "Built-in models taken out",
  modelsBack: "Offer again",
  modelsReset: "Back to Fastctx's own list",
  modelsDone: "Done",
  modelsClose: "Close",
  modelsFailed: (why) => `Could not change: ${why}`,
  idle: "Compact when idle…",
  idleTitle: "Compact when idle",
  idleOf: (who) => `Compact when idle: ${who}`,
  idleOn: "Compact when idle",
  idleOnSaid: "Runs /compact once a conversation has been idle long enough. For your own conversations only.",
  idleAfter: "Idle for",
  idleAfterSaid: "A Claude Code subscription caches for about an hour, best compacted before it lapses; an API key caches for five minutes, too short for this to compact.",
  idleMinutes: "minutes",
  idleMin: "At least",
  idleMinSaid: "A conversation with less context is not compacted.",
  idleTokens: "K tokens",
  idleTokensEach: 1_000,
  idleRange: (least, most) => `A number from ${least} to ${most}`,
  idleFailed: (why) => `Not saved: ${why}`,
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
  failed: "Failed",
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
