// What the agents page says (`world-gui.md`, 账号、机器、共享与借出): short
// labels, the longer sayings behind a pointer's rest.

import { enProblems, failure, zhProblems } from "./problem";

export interface AgentsWords {
  title: string;
  all: string;
  /** The way to a machine on the machines page. */
  onMap(machine: string): string;
  here: string;
  yours: string;
  sharedBy(owner: string): string;
  lentBy(owner: string): string;
  /** A machine of another's: what an agent lent from it does. */
  lentSaid(owner: string): string;
  online: string;
  offline: string;
  asking: string;
  notAsked: string;
  notInstalled(agent: string): string;
  loggedOut: string;
  loginUnknown: string;
  defaultHome: string;
  /** The World's tools in sessions the user opens on this computer. */
  toolsIn: string;
  toolsOld: string;
  /** What putting them in changes, behind a pointer's rest. */
  toolsInSaid(agent: "codex" | "claude"): string;
  putIn: string;
  takeOut: string;
  renew: string;
  /** Before Claude Code's tools are put in: open sessions lose their shell
   *  at once and have the World's only once opened again. */
  claudeTitle: string;
  claudeWhat: string;
  claudeGo: string;
  applyFailed(why: string): string;
  takeOutFailed(why: string): string;
  /** Codex's output limit is below what the World needs: raising it, for
   *  every tool in Codex, is the user's to ask for. */
  raise: string;
  raiseSaid: string;
  /** What apply left as it was, behind a pointer's rest. */
  kept: string;
  /** Claude working without asking for approval, in this login. */
  unattendedYes: string;
  unattendedNo: string;
  unattendedSaid: string;
  allow: string;
  withdraw: string;
  newTalk: string;
  lend: string;
  lentTo: string;
  takeBack(to: string): string;
  lendTitle(label: string, machine: string): string;
  /** Scanning a machine for its agents, and the walk of its disks. */
  scan: string;
  scanAgain: string;
  walking(directories: number): string;
  stopWalk: string;
  /** What the walk found that looks like a login, and taking one up. */
  elsewhere(count: number): string;
  takeUp(agent: string): string;
  addHome: string;
  addHomeWhere(agent: "codex" | "claude"): string;
  addHomeGo: string;
  cancel: string;
  changeFailed(why: string): string;
  borrowedGone: string;
  nothing: string;
}

export const zhAgents: AgentsWords = {
  title: "智能体",
  all: "全部",
  onMap: (machine) => `在机器图上看 ${machine}`,
  here: "本机",
  yours: "你的",
  sharedBy: (owner) => `${owner} 共享`,
  lentBy: (owner) => `来自 ${owner}`,
  lentSaid: (owner) => `${owner} 借给你；它在你的机器上干活，不接触 ${owner} 的这台电脑`,
  online: "在线",
  offline: "离线",
  asking: "读取中",
  notAsked: "无法读取",
  notInstalled: (agent) => `${agent} 未安装`,
  loggedOut: "未登录",
  loginUnknown: "已登录",
  defaultHome: "默认",
  toolsIn: "已接入",
  toolsOld: "旧版接入",
  toolsInSaid: (agent) => `新开的 ${agent === "codex" ? "Codex" : "Claude Code"} 会话会用上世界的工具`,
  putIn: "接入",
  takeOut: "拿出",
  renew: "换成新版",
  claudeTitle: "接入 Claude Code",
  claudeWhat: "已开着的 Claude Code 会话会马上用不了自带的命令行，重新打开后才有世界的工具。",
  claudeGo: "接入",
  applyFailed: failure(zhProblems, "接入失败"),
  takeOutFailed: failure(zhProblems, "拿出失败"),
  raise: "调高并接入",
  raiseSaid: "这个上限是 Codex 所有工具共用的",
  kept: "有几处保持原样",
  unattendedYes: "免审批",
  unattendedNo: "需许可",
  unattendedSaid: "世界拉起的 Claude 不经审批干活，要先在这个登录里许可一次",
  allow: "许可",
  withdraw: "撤回",
  newTalk: "新对话",
  lend: "借出",
  lentTo: "借给",
  takeBack: (to) => `收回 ${to}`,
  lendTitle: (label, machine) => `借出 ${label} · ${machine}`,
  scan: "扫描这台电脑",
  scanAgain: "重新扫描",
  walking: (directories) => `已走过 ${directories.toLocaleString("zh")} 个目录`,
  stopWalk: "停止",
  elsewhere: (count) => `别处还找到 ${count} 个`,
  takeUp: (agent) => `作为 ${agent} 候选`,
  addHome: "添加登录",
  addHomeWhere: (agent) => (agent === "codex" ? "CODEX_HOME 目录" : "CLAUDE_CONFIG_DIR 目录"),
  addHomeGo: "添加",
  cancel: "取消",
  changeFailed: failure(zhProblems, "操作失败"),
  borrowedGone: "该登录已不可用",
  nothing: "暂无机器",
};

export const enAgents: AgentsWords = {
  title: "Agents",
  all: "All",
  onMap: (machine) => `Show ${machine} on the map`,
  here: "this computer",
  yours: "yours",
  sharedBy: (owner) => `shared by ${owner}`,
  lentBy: (owner) => `from ${owner}`,
  lentSaid: (owner) => `Lent to you by ${owner}. It works on your machines and never touches ${owner}'s computer`,
  online: "online",
  offline: "offline",
  asking: "Reading",
  notAsked: "Couldn't read",
  notInstalled: (agent) => `${agent} not installed`,
  loggedOut: "Signed out",
  loginUnknown: "Signed in",
  defaultHome: "default",
  toolsIn: "Connected",
  toolsOld: "Old connection",
  toolsInSaid: (agent) => `New ${agent === "codex" ? "Codex" : "Claude Code"} sessions use the World's tools`,
  putIn: "Connect",
  takeOut: "Disconnect",
  renew: "Update",
  claudeTitle: "Connect Claude Code",
  claudeWhat: "Claude Code sessions already open lose their own shell at once and get the World's tools when opened again.",
  claudeGo: "Connect",
  applyFailed: failure(enProblems, "Couldn't connect"),
  takeOutFailed: failure(enProblems, "Couldn't take out"),
  raise: "Raise and connect",
  raiseSaid: "Every tool in Codex shares this limit",
  kept: "Some things were left as they were",
  unattendedYes: "No approvals",
  unattendedNo: "Needs consent",
  unattendedSaid: "Claude started by the World works without approvals once this login allows it",
  allow: "Allow",
  withdraw: "Withdraw",
  newTalk: "New conversation",
  lend: "Lend",
  lentTo: "Lent to",
  takeBack: (to) => `Take back from ${to}`,
  lendTitle: (label, machine) => `Lend ${label} · ${machine}`,
  scan: "Scan this computer",
  scanAgain: "Scan again",
  walking: (directories) => `${directories.toLocaleString("en")} folders looked through`,
  stopWalk: "Stop",
  elsewhere: (count) => `Also found elsewhere: ${count}`,
  takeUp: (agent) => `Add as ${agent} candidate`,
  addHome: "Add login",
  addHomeWhere: (agent) => (agent === "codex" ? "CODEX_HOME directory" : "CLAUDE_CONFIG_DIR directory"),
  addHomeGo: "Add",
  cancel: "Cancel",
  changeFailed: failure(enProblems, "Something went wrong"),
  borrowedGone: "This login is no longer available",
  nothing: "No machines yet",
};
