// What the pages about accounts say (`world-gui.md`, 账号、机器、共享与借出):
// logging in; the machines a login may use, the agents lent to it, and what
// is set on them; the account itself, with its level, its devices, who came
// into it, the machines whose agents may reach its machines, the one-time
// tokens that let another device in, its password and its projects; and the
// super administrator's page of the hub. In the user's words
// (`world-gui.md`, 用词).

import type { MemberKind } from "../world/accounts";
import type { PasswordProblem } from "./password";
import { enProblems, failure, zhProblems, type ProblemWords } from "./problem";

export interface LoginWords {
  lede: string;
  title: string;
  /** The two ways in for a computer in no World yet. */
  join: string;
  joinWhat: string;
  create: string;
  createWhat: string;
  another: string;
  hub: string;
  account: string;
  password: string;
  passwordAgain: string;
  passwordsDiffer: string;
  passwordRule: Record<PasswordProblem, string>;
  /** How strong a password that passes is, from enough to strong. */
  strength: [string, string, string];
  name: string;
  phoneName: string;
  go: string;
  going: string;
  signUp: string;
  signUpGo: string;
  /** On a phone: read the code a computer shows with the camera. */
  scan: string;
  withPassword: string;
  scanFailed: string;
  alone: string;
  aloneWhat: string;
  /** Creating a World with this computer as its server. */
  worldName: string;
  admin: string;
  reach: string;
  reachWhat: string;
  extra: string;
  extraHint: string;
  port: string;
  /** An address on a TUN, VPN or virtual switch. */
  virtual: string;
  noAddresses: string;
  noReach: string;
  createGo: string;
  createAsks: string;
  creating: string;
  joined(hub: string): string;
  created(world: string): string;
  kept: Record<"declined" | "unsupported", string>;
  keepGo: string;
  keptCommand: string;
  addDevice: string;
  enter: string;
  back: string;
  /** The logins this computer or device has already. */
  already(logins: string): string;
  /** Between the items of a list. */
  separator: string;
  placeMissing: string;
  place: string;
  problems: ProblemWords;
}

const zhPasswordRule: Record<PasswordProblem, string> = { short: "至少 8 位", "one-kind": "需包含字母、数字、符号中的两种", name: "不能包含账号名", common: "这个密码太常见" };

const enPasswordRule: Record<PasswordProblem, string> = { short: "At least 8 characters", "one-kind": "Use two of letters, digits and symbols", name: "Can't contain the account name", common: "This password is too common" };

export const zhLogin: LoginWords = {
  lede: "你的几台电脑，和电脑上的 Codex、Claude，同在一个世界。",
  title: "登录",
  join: "加入世界",
  joinWhat: "用服务器地址和账号登录",
  create: "新建世界",
  createWhat: "以这台电脑作为服务器",
  another: "登录其他账号",
  hub: "服务器地址",
  account: "账号",
  password: "密码",
  passwordAgain: "确认密码",
  passwordsDiffer: "两次输入的密码不一致",
  passwordRule: zhPasswordRule,
  strength: ["强度一般", "强度较高", "强度高"],
  name: "电脑名称",
  phoneName: "手机名称",
  go: "登录",
  going: "正在登录…",
  signUp: "注册账号",
  signUpGo: "注册并登录",
  scan: "扫码登录",
  withPassword: "使用账号密码登录",
  scanFailed: "无法扫描",
  alone: "暂不加入",
  aloneWhat: "这台电脑可以单独使用，之后随时加入或新建世界",
  worldName: "世界名称",
  admin: "管理员账号",
  reach: "连接地址",
  reachWhat: "其他电脑通过这些地址连接这台电脑",
  extra: "其他地址",
  extraHint: "例如公网 IP",
  port: "端口",
  virtual: "虚拟",
  noAddresses: "未找到网络地址，请手动填写",
  noReach: "请至少选择或填写一个地址",
  createGo: "新建",
  createAsks: "需要管理员权限",
  creating: "正在新建…",
  joined: (hub) => `已加入 ${hub}`,
  created: (world) => `${world} 已建好`,
  kept: { declined: "开机自启未设置", unsupported: "此系统不支持开机自启" },
  keepGo: "设置",
  keptCommand: "开机自启需在终端运行",
  addDevice: "添加设备",
  enter: "进入",
  back: "返回",
  already: (logins) => `已登录：${logins}`,
  separator: "、",
  placeMissing: "World 组件未安装",
  place: "安装",
  problems: zhProblems,
};

export const enLogin: LoginWords = {
  lede: "Your computers, and the Codex and Claude on them, in one World.",
  title: "Log in",
  join: "Join a World",
  joinWhat: "Log in with a server address and account",
  create: "Create a World",
  createWhat: "Use this computer as the server",
  another: "Log in to another account",
  hub: "Server address",
  account: "Account",
  password: "Password",
  passwordAgain: "Confirm password",
  passwordsDiffer: "Passwords don't match",
  passwordRule: enPasswordRule,
  strength: ["Fair", "Good", "Strong"],
  name: "Computer name",
  phoneName: "Phone name",
  go: "Log in",
  going: "Logging in…",
  signUp: "Sign up",
  signUpGo: "Sign up and log in",
  scan: "Scan to log in",
  withPassword: "Use account and password",
  scanFailed: "Couldn't scan",
  alone: "Not now",
  aloneWhat: "Use this computer on its own and join or create a World any time",
  worldName: "World name",
  admin: "Administrator account",
  reach: "Connection addresses",
  reachWhat: "Other computers connect to this one at these addresses",
  extra: "Other address",
  extraHint: "e.g. a public IP",
  port: "Port",
  virtual: "virtual",
  noAddresses: "No network address found. Enter one",
  noReach: "Choose or enter at least one address",
  createGo: "Create",
  createAsks: "Requires administrator rights",
  creating: "Creating…",
  joined: (hub) => `Joined ${hub}`,
  created: (world) => `${world} is ready`,
  kept: { declined: "Start at boot isn't set up", unsupported: "This system doesn't support start at boot" },
  keepGo: "Set up",
  keptCommand: "To start at boot, run in a terminal",
  addDevice: "Add a device",
  enter: "Enter",
  back: "Back",
  already: (logins) => `Logged in: ${logins}`,
  separator: ", ",
  placeMissing: "World components aren't installed",
  place: "Install",
  problems: enProblems,
};

export interface MachinesWords {
  title: string;
  /** The head's button that lets another machine or device in. */
  add: string;
  here: string;
  /** What the hub's super administrator may do, said behind a pointer's
   *  rest on a member's hub; the short of it, on the page. */
  superNote(admin: string): string;
  superShort(admin: string): string;
  online(since: string): string;
  onlineNow: string;
  offline(since: string | null): string;
  unknown: string;
  factsUnknown(why: string): string;
  /** A hub on the map, by how this computer reaches it; pointed at, its
   *  title and how many machines are registered on it. */
  hubOnline: string;
  hubConnecting: string;
  hubOff: string;
  hubTitle(name: string): string;
  hubMachines(n: number): string;
  /** The account's level on a hub, and what it does there. */
  levelSuper: string;
  levelMember: string;
  inviteDevice: string;
  manageHub: string;
  /** How this computer reaches a hub. */
  linkOnline(address: string): string;
  linkVia(relay: string, address: string): string;
  linkConnecting(error: string | null): string;
  linkStopped(reason: string): string;
  /** Where this computer stands on a hub. */
  registeredUnder(account: string): string;
  onlyLogins(accounts: string): string;
  via(relay: string): string;
  direct: string;
  yours: string;
  ownedBy(account: string): string;
  /** Another's machine shared with this account; a member's, used as the
   *  super administrator: the tag, and the saying behind it. */
  sharedInShort(owner: string): string;
  superInShort(owner: string): string;
  sharedIn(owner: string): string;
  superIn(owner: string): string;
  /** Whom this account shared its machine with, and lent a login to,
   *  each taken back with a press. */
  sharedTo: string;
  lentTo: string;
  takeBack(to: string): string;
  separator: string;
  /** A machine of another whose agents may reach this account's machines. */
  reaching: string;
  reachingWhat: string;
  nobody: string;
  jobs(n: number): string;
  home: string;
  hosts: string;
  asking: string;
  notAsked(why: string): string;
  notInstalled: string;
  loggedIn: string;
  loggedOut: string;
  loginUnknown: string;
  toolsIn: string;
  toolsOut: string;
  toolsOld: string;
  putIn: string;
  takeOut: string;
  renew: string;
  applied: string;
  notApplied(why: string): string;
  /** Whether Claude here may work without asking for approval. */
  unattendedYes: string;
  unattendedNo: string;
  allow: string;
  withdraw: string;
  /** A login of an agent the World did not find, added by its directory;
   *  one it found, no longer listed. */
  addHome: string;
  addHomeWhere(agent: "codex" | "claude"): string;
  addHomeGo: string;
  cancel: string;
  homeAdded(dir: string): string;
  hideHome: string;
  hideHomeWhat: string;
  homeHidden(name: string): string;
  homeNotChanged(why: string): string;
  share: string;
  lend: string;
  relays: string;
  remove: string;
  /** An agent lent to the account, on a machine of another's: the tag on
   *  the map, the one line on its panel, and the saying behind both. */
  lentShort(owner: string): string;
  borrowedNote: string;
  borrowedSaid(owner: string): string;
  /** A lent login its machine does not list. */
  borrowedGone: string;
  talk: string;
  /** On its own, logged in nowhere. */
  alone: string;
  logIn: string;
}

export const zhMachines: MachinesWords = {
  title: "机器",
  add: "添加",
  here: "本机",
  superNote: (admin) => `管理员 ${admin} 可以访问你的所有机器`,
  superShort: (admin) => `管理员 ${admin}`,
  online: (since) => `${since} 起在线`,
  onlineNow: "在线",
  offline: (since) => (since ? `${since} 起离线` : "离线"),
  unknown: "状态未知",
  factsUnknown: () => "无法获取状态",
  hubOnline: "已连接",
  hubConnecting: "连接中",
  hubOff: "未连接",
  hubTitle: (name) => `服务器 ${name}`,
  hubMachines: (n) => `${n} 台机器`,
  levelSuper: "管理员",
  levelMember: "成员",
  inviteDevice: "添加设备",
  manageHub: "管理账号",
  linkOnline: (address) => `直连 · ${address}`,
  linkVia: (relay, address) => `经 ${relay} 中转 · ${address}`,
  linkConnecting: () => "连接中",
  linkStopped: () => "已被服务器移除",
  registeredUnder: (account) => `${account} 的机器`,
  onlyLogins: (accounts) => `已登录 ${accounts}`,
  via: (relay) => `经 ${relay} 中转`,
  direct: "直连",
  yours: "你的",
  ownedBy: (account) => `${account} 的`,
  sharedInShort: (owner) => `${owner} 共享`,
  superInShort: (owner) => `成员 ${owner} 的`,
  sharedIn: (owner) => `${owner} 共享给你，可使用其上所有智能体；你在上面的操作 ${owner} 可见`,
  superIn: (owner) => `成员 ${owner} 的机器，你作为管理员可以使用；你的操作 ${owner} 可见`,
  sharedTo: "共享给",
  lentTo: "借给",
  takeBack: (to) => `收回 ${to}`,
  separator: "、",
  reaching: "可访问你的机器",
  reachingWhat: "你用过这台机器上的智能体，它们可以访问你的机器；可在账号页撤销",
  nobody: "无智能体",
  jobs: (n) => `${n} 个作业`,
  home: "家目录",
  hosts: "智能体",
  asking: "检测中…",
  notAsked: () => "无法获取智能体",
  notInstalled: "未安装",
  loggedIn: "已登录",
  loggedOut: "未登录",
  loginUnknown: "登录状态未知",
  toolsIn: "已接入 fastctx",
  toolsOut: "未接入 fastctx",
  toolsOld: "fastctx 版本过旧",
  putIn: "接入",
  takeOut: "移除",
  renew: "更新",
  applied: "已完成",
  notApplied: failure(zhProblems, "操作失败"),
  unattendedYes: "已授权自动运行",
  unattendedNo: "未授权自动运行",
  allow: "授权",
  withdraw: "撤销",
  addHome: "添加登录",
  addHomeWhere: (agent) => (agent === "codex" ? "CODEX_HOME 目录" : "CLAUDE_CONFIG_DIR 目录"),
  addHomeGo: "添加",
  cancel: "取消",
  homeAdded: (dir) => `已添加 ${dir}`,
  hideHome: "隐藏",
  hideHomeWhat: "不再列出和使用这个登录；重新添加其目录即可恢复",
  homeHidden: (name) => `已隐藏 ${name}`,
  homeNotChanged: failure(zhProblems, "操作失败"),
  share: "共享",
  lend: "借出",
  relays: "中转",
  remove: "移除",
  lentShort: (owner) => `借自 ${owner}`,
  borrowedNote: "在你的机器上工作",
  borrowedSaid: (owner) => `${owner} 借给你；它在你的机器上工作，不接触 ${owner} 的这台电脑`,
  borrowedGone: "该登录已不可用",
  talk: "新对话",
  alone: "未加入世界",
  logIn: "加入",
};

export const enMachines: MachinesWords = {
  title: "Machines",
  add: "Add",
  here: "this computer",
  superNote: (admin) => `Administrator ${admin} can access all your machines`,
  superShort: (admin) => `Admin ${admin}`,
  online: (since) => `online since ${since}`,
  onlineNow: "online",
  offline: (since) => (since ? `offline since ${since}` : "offline"),
  unknown: "status unknown",
  factsUnknown: () => "Couldn't load status",
  hubOnline: "connected",
  hubConnecting: "connecting",
  hubOff: "not connected",
  hubTitle: (name) => `Server ${name}`,
  hubMachines: (n) => `${n} machine${n === 1 ? "" : "s"}`,
  levelSuper: "administrator",
  levelMember: "member",
  inviteDevice: "Add device",
  manageHub: "Accounts",
  linkOnline: (address) => `direct · ${address}`,
  linkVia: (relay, address) => `via ${relay} · ${address}`,
  linkConnecting: () => "connecting",
  linkStopped: () => "removed by the server",
  registeredUnder: (account) => `${account}'s machine`,
  onlyLogins: (accounts) => `signed in to ${accounts}`,
  via: (relay) => `via ${relay}`,
  direct: "direct",
  yours: "yours",
  ownedBy: (account) => `${account}'s`,
  sharedInShort: (owner) => `shared by ${owner}`,
  superInShort: (owner) => `member ${owner}'s`,
  sharedIn: (owner) => `Shared with you by ${owner}, with all its agents; ${owner} can see what you do on it`,
  superIn: (owner) => `Member ${owner}'s machine, available to you as administrator; ${owner} can see what you do on it`,
  sharedTo: "Shared with",
  lentTo: "Lent to",
  takeBack: (to) => `Revoke ${to}`,
  separator: ", ",
  reaching: "can access your machines",
  reachingWhat: "You used an agent on this machine, so its agents can access your machines; revoke on the Account page",
  nobody: "No agents",
  jobs: (n) => `${n} job${n === 1 ? "" : "s"}`,
  home: "Home",
  hosts: "Agents",
  asking: "Checking…",
  notAsked: () => "Couldn't load agents",
  notInstalled: "not installed",
  loggedIn: "signed in",
  loggedOut: "not signed in",
  loginUnknown: "sign-in unknown",
  toolsIn: "fastctx connected",
  toolsOut: "fastctx not connected",
  toolsOld: "fastctx outdated",
  putIn: "Connect",
  takeOut: "Remove",
  renew: "Update",
  applied: "Done",
  notApplied: failure(enProblems, "Couldn't complete"),
  unattendedYes: "allowed to run unattended",
  unattendedNo: "not allowed to run unattended",
  allow: "Allow",
  withdraw: "Revoke",
  addHome: "Add login",
  addHomeWhere: (agent) => (agent === "codex" ? "CODEX_HOME directory" : "CLAUDE_CONFIG_DIR directory"),
  addHomeGo: "Add",
  cancel: "Cancel",
  homeAdded: (dir) => `Added ${dir}`,
  hideHome: "Hide",
  hideHomeWhat: "Stop listing and using this login; add its directory again to restore it",
  homeHidden: (name) => `Hid ${name}`,
  homeNotChanged: failure(enProblems, "Couldn't complete"),
  share: "Share",
  lend: "Lend",
  relays: "Relays",
  remove: "Remove",
  lentShort: (owner) => `lent by ${owner}`,
  borrowedNote: "Works on your machines",
  borrowedSaid: (owner) => `Lent to you by ${owner}; it works on your machines and never touches ${owner}'s computer`,
  borrowedGone: "Login unavailable",
  talk: "New conversation",
  alone: "Not in a World",
  logIn: "Join",
};

/** The sheets on a machine: whom its owner shares it with, whom it lends
 *  one login of an agent on it, the relays it connects through, and
 *  removing it. */
export interface AccessWords {
  shareTitle(machine: string): string;
  shareWhat: string;
  shareSees: string;
  sharedTo: string;
  shareGo: string;
  lendTitle(login: string, machine: string): string;
  lendWhat: string;
  lendSees: string;
  lentTo: string;
  lendGo: string;
  nobodyYet: string;
  to: string;
  toHint: string;
  takeBack: string;
  relaysTitle(machine: string): string;
  relaysWhat: string;
  relayLabel: string;
  relayAddresses: string;
  addRelay: string;
  removeRelay: string;
  noRelays: string;
  hubLast: string;
  badAddress(text: string): string;
  needsLabel: string;
  relaysGo: string;
  removeTitle(machine: string): string;
  removeWhat(machine: string): string;
  removeGo: string;
  reason: string;
  reasonDefault: string;
  working: string;
  close: string;
  failed(why: string): string;
}

export const zhAccess: AccessWords = {
  shareTitle: (machine) => `共享 ${machine}`,
  shareWhat: "对方可以使用这台机器和上面的所有智能体。",
  shareSees: "对方的操作会在这台机器上真实运行，请只共享给信任的人。",
  sharedTo: "已共享给",
  shareGo: "共享",
  lendTitle: (login, machine) => `借出 ${machine} 上的 ${login}`,
  lendWhat: "只借出这个登录，不含机器；它在对方的机器上为对方工作。",
  lendSees: "会使用这个登录的订阅额度，运行过程在你的机器上可见、可停止。",
  lentTo: "已借给",
  lendGo: "借出",
  nobodyYet: "暂无",
  to: "账号",
  toHint: "对方的账号名",
  takeBack: "收回",
  relaysTitle: (machine) => `${machine} 的中转`,
  relaysWhat: "按顺序尝试中转地址，都不通时直连服务器。中转只转发，无法读取内容。",
  relayLabel: "名称",
  relayAddresses: "地址",
  addRelay: "添加中转",
  removeRelay: "删除",
  noRelays: "直连服务器",
  hubLast: "最后直连服务器",
  badAddress: (text) => `「${text}」不是有效的 IP:端口`,
  needsLabel: "请为每个中转命名",
  relaysGo: "保存",
  removeTitle: (machine) => `移除 ${machine}？`,
  removeWhat: (machine) => `${machine} 将断开服务器并退出所有账号，共享与借出随之失效。机器上的数据不受影响。`,
  removeGo: "移除",
  reason: "原因",
  reasonDefault: "removed",
  working: "处理中…",
  close: "关闭",
  failed: failure(zhProblems, "操作失败"),
};

export const enAccess: AccessWords = {
  shareTitle: (machine) => `Share ${machine}`,
  shareWhat: "They can use this machine and every agent on it.",
  shareSees: "What they do runs for real on this machine. Share only with people you trust.",
  sharedTo: "Shared with",
  shareGo: "Share",
  lendTitle: (login, machine) => `Lend ${login} on ${machine}`,
  lendWhat: "Only this login is lent, not the machine; it works for them on their machines.",
  lendSees: "It uses this login's subscription. You can see and stop it on your machine.",
  lentTo: "Lent to",
  lendGo: "Lend",
  nobodyYet: "None yet",
  to: "Account",
  toHint: "Their account name",
  takeBack: "Revoke",
  relaysTitle: (machine) => `Relays for ${machine}`,
  relaysWhat: "Relays are tried in order, then the server directly. A relay only forwards and can't read the traffic.",
  relayLabel: "Name",
  relayAddresses: "Address",
  addRelay: "Add relay",
  removeRelay: "Delete",
  noRelays: "Connects directly",
  hubLast: "Then the server directly",
  badAddress: (text) => `“${text}” isn't a valid IP:port`,
  needsLabel: "Name each relay",
  relaysGo: "Save",
  removeTitle: (machine) => `Remove ${machine}?`,
  removeWhat: (machine) => `${machine} will disconnect from the server and sign out of every account; its shares and loans end. Data on it is untouched.`,
  removeGo: "Remove",
  reason: "Reason",
  reasonDefault: "removed",
  working: "Working…",
  close: "Close",
  failed: failure(enProblems, "Couldn't complete"),
};

export interface AccountWords {
  title: string;
  hub: string;
  account: string;
  level: Record<"super" | "member", string>;
  /** On a member's page: what the super administrator may do. */
  superNote(admin: string): string;
  thisOne: string;
  /** Who came into the account. */
  arrived(kind: string, name: string, at: string): string;
  me: string;
  notMe: string;
  notMeWhat(name: string): string;
  devices: string;
  devicesWhat: string;
  kinds: Record<MemberKind, string>;
  machineTag: string;
  key(start: string): string;
  added(at: string): string;
  onlineNow: string;
  lastSeen(at: string): string;
  neverSeen: string;
  removeDevice: string;
  removeDeviceTitle(name: string): string;
  removeDeviceWhat(name: string, account: string, machine: boolean): string;
  removeDeviceGo: string;
  invite: string;
  inviteWhat: string;
  invitePhone: string;
  inviteComputer: string;
  inviteServer: string;
  /** The machines of others whose agents may reach the account's. */
  reaching: string;
  reachingWhat: string;
  reachingNone: string;
  reachingLine(machine: string, owner: string | null): string;
  reachingSince(at: string): string;
  stopReaching: string;
  stopReachingTitle(machine: string): string;
  stopReachingWhat(machine: string, owner: string | null): string;
  stopReachingGo: string;
  projects: string;
  noProjects: string;
  sharedWith: string;
  sharedBy(owner: string): string;
  share: string;
  shareTitle(project: string): string;
  shareWith: string;
  shareGo: string;
  unshare(account: string): string;
  password: string;
  passwordWhat: string;
  changePassword: string;
  passwordTitle: string;
  newPassword: string;
  passwordAgain: string;
  passwordsDiffer: string;
  passwordRule: Record<PasswordProblem, string>;
  passwordGo: string;
  passwordDone: string;
  logout: string;
  logoutTitle(login: string): string;
  logoutWhat(login: string, registered: boolean): string;
  logoutGo: string;
  adminPage: string;
  another: string;
  /** Logged in nowhere. */
  none: string;
  logIn: string;
  working: string;
  close: string;
  failed(why: string): string;
}

export const zhAccount: AccountWords = {
  title: "账号",
  hub: "服务器",
  account: "账号",
  level: { super: "管理员", member: "成员" },
  superNote: (admin) => `管理员 ${admin} 可以访问你的所有机器`,
  thisOne: "本机",
  arrived: (kind, name, at) => `${at} · 新${kind} ${name}`,
  me: "是我",
  notMe: "不是我",
  notMeWhat: (name) => `移除 ${name}，它将无法再使用这个账号。`,
  devices: "已登录的设备",
  devicesWhat: "丢失的设备可以在这里移除",
  kinds: { computer: "电脑", phone: "手机", web: "网页 AI" },
  machineTag: "机器",
  key: (start) => `密钥 ${start}`,
  added: (at) => `${at} 登录`,
  onlineNow: "在线",
  lastSeen: (at) => `${at} 在线`,
  neverSeen: "从未连接",
  removeDevice: "移除",
  removeDeviceTitle: (name) => `移除 ${name}？`,
  removeDeviceWhat: (name, account, machine) =>
    machine
      ? `${name} 将断开服务器并退出所有账号，共享与借出随之失效。机器上的数据不受影响。`
      : `${name} 将退出 ${account}。`,
  removeDeviceGo: "移除",
  invite: "添加设备",
  inviteWhat: "手机扫码，电脑用服务器地址和账号登录，Linux 主机运行一行命令",
  invitePhone: "手机",
  inviteComputer: "电脑",
  inviteServer: "Linux 主机",
  reaching: "可访问你的机器的智能体",
  reachingWhat: "你用过其他人机器上的智能体后，它们可以访问你的机器",
  reachingNone: "暂无",
  reachingLine: (machine, owner) => (owner ? `${owner} 的 ${machine}` : machine),
  reachingSince: (at) => `${at} 起`,
  stopReaching: "撤销",
  stopReachingTitle: (machine) => `撤销 ${machine} 的访问？`,
  stopReachingWhat: (machine) => `${machine} 上的智能体将无法再访问你的机器；下次使用时会重新获得访问。`,
  stopReachingGo: "撤销",
  projects: "项目",
  noProjects: "暂无项目",
  sharedWith: "共享给",
  sharedBy: (owner) => `${owner} 共享`,
  share: "共享",
  shareTitle: (project) => `共享「${project}」`,
  shareWith: "账号",
  shareGo: "共享",
  unshare: (account) => `取消共享给 ${account}`,
  password: "密码",
  passwordWhat: "已登录的设备不受影响",
  changePassword: "修改密码",
  passwordTitle: "修改密码",
  newPassword: "新密码",
  passwordAgain: "确认新密码",
  passwordsDiffer: "两次输入的密码不一致",
  passwordRule: zhPasswordRule,
  passwordGo: "保存",
  passwordDone: "密码已修改",
  logout: "退出账号",
  logoutTitle: (login) => `退出 ${login}？`,
  logoutWhat: (login, registered) =>
    registered
      ? `这台电脑将退出 ${login}，不再作为它的机器，共享与借出随之失效。电脑上的数据不受影响。`
      : `这台设备将退出 ${login}。`,
  logoutGo: "退出",
  adminPage: "管理服务器",
  another: "添加账号",
  none: "未加入世界",
  logIn: "加入",
  working: "处理中…",
  close: "关闭",
  failed: failure(zhProblems, "操作失败"),
};

export const enAccount: AccountWords = {
  title: "Account",
  hub: "Server",
  account: "Account",
  level: { super: "administrator", member: "member" },
  superNote: (admin) => `Administrator ${admin} can access all your machines`,
  thisOne: "this computer",
  arrived: (kind, name, at) => `${at} · new ${kind} ${name}`,
  me: "It's me",
  notMe: "Not me",
  notMeWhat: (name) => `Remove ${name}? It will no longer be able to use this account.`,
  devices: "Signed-in devices",
  devicesWhat: "Remove a lost device here",
  kinds: { computer: "computer", phone: "phone", web: "web AI" },
  machineTag: "machine",
  key: (start) => `key ${start}`,
  added: (at) => `signed in ${at}`,
  onlineNow: "online",
  lastSeen: (at) => `online ${at}`,
  neverSeen: "never connected",
  removeDevice: "Remove",
  removeDeviceTitle: (name) => `Remove ${name}?`,
  removeDeviceWhat: (name, account, machine) =>
    machine
      ? `${name} will disconnect from the server and sign out of every account; its shares and loans end. Data on it is untouched.`
      : `${name} will be signed out of ${account}.`,
  removeDeviceGo: "Remove",
  invite: "Add device",
  inviteWhat: "Phones scan a code, computers sign in with the server address and account, Linux hosts run one command",
  invitePhone: "Phone",
  inviteComputer: "Computer",
  inviteServer: "Linux host",
  reaching: "Agents with access to your machines",
  reachingWhat: "Once you use an agent on someone else's machine, its agents can access your machines",
  reachingNone: "None",
  reachingLine: (machine, owner) => (owner ? `${owner}'s ${machine}` : machine),
  reachingSince: (at) => `since ${at}`,
  stopReaching: "Revoke",
  stopReachingTitle: (machine) => `Revoke access for ${machine}?`,
  stopReachingWhat: (machine) => `Agents on ${machine} will lose access to your machines until you use one again.`,
  stopReachingGo: "Revoke",
  projects: "Projects",
  noProjects: "No projects yet",
  sharedWith: "Shared with",
  sharedBy: (owner) => `shared by ${owner}`,
  share: "Share",
  shareTitle: (project) => `Share “${project}”`,
  shareWith: "Account",
  shareGo: "Share",
  unshare: (account) => `Stop sharing with ${account}`,
  password: "Password",
  passwordWhat: "Signed-in devices stay signed in",
  changePassword: "Change password",
  passwordTitle: "Change password",
  newPassword: "New password",
  passwordAgain: "Confirm new password",
  passwordsDiffer: "Passwords don't match",
  passwordRule: enPasswordRule,
  passwordGo: "Save",
  passwordDone: "Password changed",
  logout: "Sign out",
  logoutTitle: (login) => `Sign out of ${login}?`,
  logoutWhat: (login, registered) =>
    registered
      ? `This computer will sign out of ${login} and stop being its machine; its shares and loans end. Data on it is untouched.`
      : `This device will sign out of ${login}.`,
  logoutGo: "Sign out",
  adminPage: "Manage server",
  another: "Add account",
  none: "Not in a World",
  logIn: "Join",
  working: "Working…",
  close: "Close",
  failed: failure(enProblems, "Couldn't complete"),
};

/** The sheet that adds another device to an account. */
export interface TokenWords {
  title: string;
  kinds: Record<"computer" | "phone" | "server", string>;
  hub: string;
  account: string;
  name: string;
  nameAuto: string;
  valid: string;
  hours(n: number): string;
  makeCode: string;
  makeCommand: string;
  making: string;
  qr: string;
  command: string;
  copy: string;
  copied: string;
  until(at: string): string;
  again: string;
  close: string;
  problems: ProblemWords;
}

export const zhToken: TokenWords = {
  title: "添加设备",
  kinds: { computer: "电脑", phone: "手机", server: "Linux 主机" },
  hub: "服务器地址",
  account: "账号",
  name: "主机名称",
  nameAuto: "自动",
  valid: "有效期",
  hours: (n) => (n < 24 ? `${n} 小时` : `${n / 24} 天`),
  makeCode: "生成二维码",
  makeCommand: "生成命令",
  making: "正在生成…",
  qr: "登录二维码",
  command: "在主机终端中运行",
  copy: "复制",
  copied: "已复制",
  until: (at) => `${at} 前有效，仅可使用一次`,
  again: "重新生成",
  close: "关闭",
  problems: zhProblems,
};

export const enToken: TokenWords = {
  title: "Add a device",
  kinds: { computer: "Computer", phone: "Phone", server: "Linux host" },
  hub: "Server address",
  account: "Account",
  name: "Host name",
  nameAuto: "automatic",
  valid: "Valid for",
  hours: (n) => (n < 24 ? `${n} hour${n === 1 ? "" : "s"}` : `${n / 24} day${n === 24 ? "" : "s"}`),
  makeCode: "Create QR code",
  makeCommand: "Create command",
  making: "Creating…",
  qr: "Login QR code",
  command: "Run in the host's terminal",
  copy: "Copy",
  copied: "Copied",
  until: (at) => `Valid until ${at}, once`,
  again: "Create again",
  close: "Close",
  problems: enProblems,
};

export interface AdminWords {
  title: string;
  hub(name: string): string;
  /** Being the super administrator: what it means, and handing it on. */
  superPart: string;
  superWhat: string;
  transfer: string;
  transferTitle: string;
  transferWhat: string;
  transferTo: string;
  transferGo: string;
  noOther: string;
  selfRegistration: string;
  selfRegistrationWhat: string;
  accounts: string;
  opened(at: string): string;
  superTag: string;
  locked: string;
  you: string;
  lock: string;
  unlock: string;
  resetPassword: string;
  resetTitle(account: string): string;
  resetWhat: string;
  resetGo: string;
  deleteAccount: string;
  deleteTitle(account: string): string;
  deleteWhat(account: string, machines: string[], projects: string[]): string;
  deleteGo: string;
  openAccount: string;
  openTitle: string;
  openWhat: string;
  name: string;
  nameHint: string;
  password: string;
  passwordAgain: string;
  passwordsDiffer: string;
  passwordRule: Record<PasswordProblem, string>;
  openGo: string;
  projects: string;
  owner: string;
  handTo: string;
  handTitle(project: string): string;
  handWhat: string;
  handGo: string;
  notAdmin: string;
  working: string;
  close: string;
  failed(why: string): string;
}

export const zhAdmin: AdminWords = {
  title: "管理服务器",
  hub: (name) => `服务器 ${name}`,
  superPart: "管理员",
  superWhat: "可以使用服务器上的所有机器；管理员只有一位，可以转交",
  transfer: "转交",
  transferTitle: "转交管理员",
  transferWhat: "转交后你将成为成员，对方可以使用服务器上的所有机器。",
  transferTo: "转交给",
  transferGo: "转交",
  noOther: "没有其他账号",
  selfRegistration: "开放注册",
  selfRegistrationWhat: "能连接服务器的人都可以自行注册，注册的账号为成员",
  accounts: "账号",
  opened: (at) => `${at} 创建`,
  superTag: "管理员",
  locked: "已锁定",
  you: "你",
  lock: "锁定",
  unlock: "解锁",
  resetPassword: "重设密码",
  resetTitle: (account) => `重设 ${account} 的密码`,
  resetWhat: "已登录的设备不受影响",
  resetGo: "重设",
  deleteAccount: "删除",
  deleteTitle: (account) => `删除 ${account}？`,
  deleteWhat: (_account, machines, projects) =>
    [
      "此操作无法撤销。",
      machines.length ? `名下机器 ${machines.join("、")} 将被移除。` : "",
      "共享与借出随之失效。",
      projects.length ? `项目 ${projects.join("、")} 将归档保留，可转交给其他账号。` : "",
    ].join(""),
  deleteGo: "删除",
  openAccount: "新建账号",
  openTitle: "新建账号",
  openWhat: "新账号为成员",
  name: "账号名",
  nameHint: "小写字母开头，可含数字和连字符",
  password: "密码",
  passwordAgain: "确认密码",
  passwordsDiffer: "两次输入的密码不一致",
  passwordRule: zhPasswordRule,
  openGo: "创建",
  projects: "项目",
  owner: "所有者",
  handTo: "转交",
  handTitle: (project) => `转交「${project}」`,
  handWhat: "项目及其记录将归对方所有。",
  handGo: "转交",
  notAdmin: "当前登录的账号都不是管理员",
  working: "处理中…",
  close: "关闭",
  failed: failure(zhProblems, "操作失败"),
};

export const enAdmin: AdminWords = {
  title: "Manage server",
  hub: (name) => `Server ${name}`,
  superPart: "Administrator",
  superWhat: "Can use every machine on the server; there is one administrator, and the role can be transferred",
  transfer: "Transfer",
  transferTitle: "Transfer administrator",
  transferWhat: "You'll become a member, and they'll be able to use every machine on the server.",
  transferTo: "Transfer to",
  transferGo: "Transfer",
  noOther: "No other accounts",
  selfRegistration: "Open sign-up",
  selfRegistrationWhat: "Anyone who can reach the server can sign up; new accounts are members",
  accounts: "Accounts",
  opened: (at) => `created ${at}`,
  superTag: "administrator",
  locked: "locked",
  you: "you",
  lock: "Lock",
  unlock: "Unlock",
  resetPassword: "Reset password",
  resetTitle: (account) => `Reset password for ${account}`,
  resetWhat: "Signed-in devices stay signed in",
  resetGo: "Reset",
  deleteAccount: "Delete",
  deleteTitle: (account) => `Delete ${account}?`,
  deleteWhat: (_account, machines, projects) =>
    [
      "This can't be undone. ",
      machines.length ? `Its machines ${machines.join(", ")} will be removed. ` : "",
      "Its shares and loans end. ",
      projects.length ? `Projects ${projects.join(", ")} are archived and can be transferred.` : "",
    ].join(""),
  deleteGo: "Delete",
  openAccount: "New account",
  openTitle: "New account",
  openWhat: "New accounts are members",
  name: "Account name",
  nameHint: "Starts with a lowercase letter; digits and hyphens allowed",
  password: "Password",
  passwordAgain: "Confirm password",
  passwordsDiffer: "Passwords don't match",
  passwordRule: enPasswordRule,
  openGo: "Create",
  projects: "Projects",
  owner: "Owner",
  handTo: "Transfer",
  handTitle: (project) => `Transfer “${project}”`,
  handWhat: "The project and its records will belong to them.",
  handGo: "Transfer",
  notAdmin: "None of your accounts is an administrator",
  working: "Working…",
  close: "Close",
  failed: failure(enProblems, "Couldn't complete"),
};
