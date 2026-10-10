// What the pages about accounts say (`world-gui.md`, 账号、机器、共享与借出):
// logging in; the machines a login may use, the agents lent to it, and what
// is set on them; the account itself, with its level, its devices, who came
// into it, the machines whose agents may reach its machines, the one-time
// tokens that let another device in, its password and its projects; and the
// super administrator's page of the hub. In the user's words
// (`world-gui.md`, 用词).

import type { MemberKind } from "../world/accounts";
import type { PasswordProblem } from "./password";
import type { ProblemWords } from "./problem";

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
  register: string;
  registerWhat: string;
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
  hostsWhat: string;
  codex: string;
  claude: string;
  applyGo: string;
  applying: string;
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

export const zhProblems: ProblemWords = {
  wrongPassword: "账号或密码不正确",
  tooManyTries: "尝试次数过多，请稍后再试",
  locked: "该账号已被锁定，请联系管理员",
  unreachable: "无法连接服务器，请检查地址和网络",
  nameTaken: "这个名称已被使用",
  accountTaken: "这个账号已存在",
  noSelfRegistration: "该服务器不开放注册，请联系管理员",
  codeUsed: "二维码已使用过",
  codeExpired: "二维码已过期",
  codeWrong: "二维码无效",
  weakPassword: "密码不符合要求",
  somethingElse: "操作未完成",
};

export const enProblems: ProblemWords = {
  wrongPassword: "Wrong account or password",
  tooManyTries: "Too many attempts. Try again later",
  locked: "This account is locked. Contact the administrator",
  unreachable: "Can't reach the server. Check the address and network",
  nameTaken: "That name is already in use",
  accountTaken: "That account already exists",
  noSelfRegistration: "This server doesn't allow sign-up. Contact the administrator",
  codeUsed: "This code has already been used",
  codeExpired: "This code has expired",
  codeWrong: "This code isn't valid",
  weakPassword: "The password doesn't meet the requirements",
  somethingElse: "Something went wrong",
};

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
  passwordRule: { short: "至少 8 位", "one-kind": "需包含字母、数字、符号中的两种", name: "不能包含账号名", common: "这个密码太常见" },
  strength: ["强度一般", "强度较高", "强度高"],
  register: "将这台电脑添加为我的机器",
  registerWhat: "智能体可以在这台电脑上运行和工作；不勾选则只登录",
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
  hostsWhat: "让你在这台电脑上打开的 Codex 和 Claude Code 使用世界的工具",
  codex: "Codex 尚未接入",
  claude: "Claude Code 尚未接入",
  applyGo: "接入",
  applying: "正在接入…",
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
  passwordRule: { short: "At least 8 characters", "one-kind": "Use two of letters, digits and symbols", name: "Can't contain the account name", common: "This password is too common" },
  strength: ["Fair", "Good", "Strong"],
  register: "Add this computer as my machine",
  registerWhat: "Agents can run and work on this computer; leave unchecked to only log in",
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
  hostsWhat: "Give the Codex and Claude Code you open on this computer the World's tools",
  codex: "Codex isn't connected",
  claude: "Claude Code isn't connected",
  applyGo: "Connect",
  applying: "Connecting…",
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
  here: "这台",
  superNote: (admin) => `这个中枢的超级管理员是 ${admin}：你的每一台机器，${admin} 都看得见、用得了。`,
  superShort: (admin) => `${admin} 能用你的机器`,
  online: (since) => `在线，${since} 起`,
  onlineNow: "在线",
  offline: (since) => (since ? `离线，${since} 起` : "离线"),
  unknown: "不知道在不在线",
  factsUnknown: (why) => `问不到它的情况：${why}`,
  hubOnline: "连着",
  hubConnecting: "正在连",
  hubOff: "没连上",
  hubTitle: (name) => `中枢 ${name}`,
  hubMachines: (n) => `${n} 台机器登记在这里`,
  levelSuper: "超级管理员",
  levelMember: "成员",
  inviteDevice: "加一台设备…",
  manageHub: "管理账号",
  linkOnline: (address) => `直连着，${address}`,
  linkVia: (relay, address) => `经转接站 ${relay} 连着，${address}`,
  linkConnecting: (error) => (error ? `正在连，上一次没连上：${error}` : "正在连"),
  linkStopped: (reason) => `被中枢移除了：${reason}`,
  registeredUnder: (account) => `登记成了 ${account} 的机器`,
  onlyLogins: (accounts) => `只登录着 ${accounts}，没有登记成谁的机器`,
  via: (relay) => `经转接站 ${relay} 连着中枢`,
  direct: "直连中枢",
  yours: "你的",
  ownedBy: (account) => `${account} 的`,
  sharedInShort: (owner) => `${owner} 共享的`,
  superInShort: (owner) => `成员 ${owner} 的`,
  sharedIn: (owner) => `${owner} 把它共享给了你：能在上面干活，也能用上面的全部智能体；你在上面做的事、留在那里的输出，${owner} 都看得见。`,
  superIn: (owner) => `这是成员 ${owner} 的机器，你是超级管理员，所以能用它；你在上面做的事 ${owner} 都看得见。`,
  sharedTo: "共享给",
  lentTo: "借给",
  takeBack: (to) => `从 ${to} 那里收回`,
  separator: "、",
  reaching: "能访问你的机器",
  reachingWhat: "你用过它上面的智能体，所以它上面的智能体能访问你的机器；在账号页上可以拿掉。",
  nobody: "此刻没有智能体",
  jobs: (n) => `${n} 个作业在跑`,
  home: "家目录",
  hosts: "上面的智能体",
  asking: "正在问它上面的智能体…",
  notAsked: (why) => `问不到它上面的智能体：${why}`,
  notInstalled: "没装",
  loggedIn: "已登录",
  loggedOut: "没登录",
  loginUnknown: "登没登录说不准",
  toolsIn: "你在这里开的会话用着世界的工具",
  toolsOut: "你在这里开的会话没接入世界的工具",
  toolsOld: "接入的还是旧版 fastctx",
  putIn: "接入",
  takeOut: "拿出",
  renew: "换成新版",
  applied: "改好了",
  notApplied: (why) => `没改成：${why}`,
  unattendedYes: "已许可它不经审批干活",
  unattendedNo: "还没许可它不经审批干活，世界请不来它",
  allow: "许可…",
  withdraw: "撤回",
  addHome: "添加一个登录…",
  addHomeWhere: (agent) =>
    agent === "codex" ? "这台机器上的目录，就是 CODEX_HOME 指的那个" : "这台机器上的目录，就是 CLAUDE_CONFIG_DIR 指的那个",
  addHomeGo: "添加",
  cancel: "算了",
  homeAdded: (dir) => `加上了 ${dir}`,
  hideHome: "不再列出",
  hideHomeWhat: "世界不再列出、也不再用这个登录；在「添加一个登录」里写回它的目录，它就回来",
  homeHidden: (name) => `不再列出 ${name}；在「添加一个登录」里写回它的目录，它就回来`,
  homeNotChanged: (why) => `没改成：${why}`,
  share: "共享…",
  lend: "借给…",
  relays: "转接站…",
  remove: "移除…",
  lentShort: (owner) => `借自 ${owner}`,
  borrowedNote: "在你的机器上干活，不碰这一台",
  borrowedSaid: (owner) => `${owner} 借给你的。它跑在 ${owner} 的这台机器上，却碰不到这台电脑：替你干活时，它的每条命令都写明去你的哪台机器。`,
  borrowedGone: "这台机器上找不到这个登录了",
  talk: "开一段对话",
  alone: "这台电脑还没有登录任何账号：它独自一台，只有它自己。",
  logIn: "登录…",
};

export const enMachines: MachinesWords = {
  title: "Machines",
  add: "Add",
  here: "this one",
  superNote: (admin) => `${admin} is this hub's super administrator, and sees and may use every machine of yours.`,
  superShort: (admin) => `${admin} may use your machines`,
  online: (since) => `online since ${since}`,
  onlineNow: "online",
  offline: (since) => (since ? `offline since ${since}` : "offline"),
  unknown: "not known whether online",
  factsUnknown: (why) => `could not be asked: ${why}`,
  hubOnline: "connected",
  hubConnecting: "connecting",
  hubOff: "not connected",
  hubTitle: (name) => `Hub ${name}`,
  hubMachines: (n) => `${n} machine${n === 1 ? "" : "s"} registered here`,
  levelSuper: "super administrator",
  levelMember: "member",
  inviteDevice: "Add a device…",
  manageHub: "Accounts",
  linkOnline: (address) => `connected directly, ${address}`,
  linkVia: (relay, address) => `connected through the relay ${relay}, ${address}`,
  linkConnecting: (error) => (error ? `connecting; the last try failed: ${error}` : "connecting"),
  linkStopped: (reason) => `removed by the hub: ${reason}`,
  registeredUnder: (account) => `registered as a machine of ${account}`,
  onlyLogins: (accounts) => `logged in to ${accounts} only, registered as nobody's machine`,
  via: (relay) => `reaches the hub through the relay ${relay}`,
  direct: "reaches the hub directly",
  yours: "yours",
  ownedBy: (account) => `${account}'s`,
  sharedInShort: (owner) => `shared by ${owner}`,
  superInShort: (owner) => `member ${owner}'s`,
  sharedIn: (owner) => `${owner} shared it with you: you may work on it and use every agent on it, and ${owner} sees what you do there and the output you leave.`,
  superIn: (owner) => `A machine of the member ${owner}; you may use it as the super administrator, and ${owner} sees what you do there.`,
  sharedTo: "Shared with",
  lentTo: "Lent to",
  takeBack: (to) => `Take back from ${to}`,
  separator: ", ",
  reaching: "can reach your machines",
  reachingWhat: "You used an agent on it, so its agents can reach your machines; you can take that back on the account page.",
  nobody: "No agent right now",
  jobs: (n) => `${n} job${n === 1 ? "" : "s"} running`,
  home: "Home",
  hosts: "Its agents",
  asking: "Asking about its agents…",
  notAsked: (why) => `Its agents could not be asked about: ${why}`,
  notInstalled: "not installed",
  loggedIn: "logged in",
  loggedOut: "not logged in",
  loginUnknown: "login not known",
  toolsIn: "sessions you open here have the World's tools",
  toolsOut: "sessions you open here don't have the World's tools",
  toolsOld: "still has the old fastctx",
  putIn: "Put in",
  takeOut: "Take out",
  renew: "Use the new one",
  applied: "Done",
  notApplied: (why) => `Not done: ${why}`,
  unattendedYes: "allowed to work without asking",
  unattendedNo: "not yet allowed to work without asking; the World cannot bring it in",
  allow: "Allow…",
  withdraw: "Withdraw",
  addHome: "Add a login…",
  addHomeWhere: (agent) =>
    agent === "codex" ? "A directory on this machine, the one CODEX_HOME names" : "A directory on this machine, the one CLAUDE_CONFIG_DIR names",
  addHomeGo: "Add",
  cancel: "Cancel",
  homeAdded: (dir) => `Added ${dir}`,
  hideHome: "Stop listing",
  hideHomeWhat: "The World no longer lists or uses this login; write its directory under Add a login and it comes back",
  homeHidden: (name) => `No longer listing ${name}; write its directory under Add a login and it comes back`,
  homeNotChanged: (why) => `Not changed: ${why}`,
  share: "Share…",
  lend: "Lend…",
  relays: "Relays…",
  remove: "Remove…",
  lentShort: (owner) => `lent by ${owner}`,
  borrowedNote: "Works on your machines, never on this one",
  borrowedSaid: (owner) => `Lent to you by ${owner}. It runs on this machine of ${owner}'s, which it cannot touch: working for you, each of its commands names one of your machines.`,
  borrowedGone: "this machine no longer has this login",
  talk: "Talk to it",
  alone: "This computer is logged in to no account: it is on its own, a World of one.",
  logIn: "Log in…",
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
  shareAfter: string;
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
  relayAddressesHint: string;
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
  shareTitle: (machine) => `把 ${machine} 共享给别的账号`,
  shareWhat: "共享出去，对方就能在这台机器上干活，也能用上面登录着的全部智能体，包括以后才登录上来的。",
  shareSees: "对方在这台机器上做的一切你都看得见；它跑的命令是这台机器上真实的进程。只共享给信得过的人。",
  sharedTo: "已经共享给",
  shareGo: "共享",
  shareAfter: "只想给一个智能体、不给机器？在机器页上那个登录旁边点「借给…」。",
  lendTitle: (login, machine) => `把 ${machine} 上的 ${login} 借给别的账号`,
  lendWhat: "借出去的只是这一个登录，不是机器：对方能和它说话、请它当同事，它替对方在对方自己的机器上干活。",
  lendSees: "借出去的是这个登录的整个账号，订阅额度跟着一起用；它替对方干的一切都在你的这台机器上，你看得见、停得掉。只借给信得过的人。",
  lentTo: "已经借给",
  lendGo: "借出",
  nobodyYet: "还没给谁。",
  to: "对方的账号",
  toHint: "对方在这个中枢上的账号名",
  takeBack: "收回",
  relaysTitle: (machine) => `${machine} 经哪些转接站连中枢`,
  relaysWhat: "转接站是一个只转发到中枢的端口，比如另一台线路更好的机器上的 fastctx relay。这台机器按顺序先试这些地址，都不通再直连中枢；它读不到经过的内容。",
  relayLabel: "叫什么",
  relayAddresses: "地址",
  relayAddressesHint: "IP 加端口，几个就用逗号隔开，比如 203.0.113.5:7443",
  addRelay: "加一个转接站",
  removeRelay: "删掉这一个",
  noRelays: "现在直连中枢。",
  hubLast: "都不通时直连中枢。",
  badAddress: (text) => `「${text}」不是 IP 加端口。`,
  needsLabel: "每个转接站都要起个名字。",
  relaysGo: "就这样连",
  removeTitle: (machine) => `移除 ${machine}`,
  removeWhat: (machine) =>
    `${machine} 从中枢上除名：不再连这个中枢，它在这里登录的账号都随之退出，它的共享与借出一并作废。它身上的东西不会被删掉；要回来，得在它上面重新登录。`,
  removeGo: "移除它",
  reason: "原因",
  reasonDefault: "removed",
  working: "稍等…",
  close: "关闭",
  failed: (why) => `没办成：${why}`,
};

export const enAccess: AccessWords = {
  shareTitle: (machine) => `Share ${machine} with another account`,
  shareWhat: "Shared, it lets the other account work on this machine and use every agent logged in on it, including those that log in later.",
  shareSees: "You see everything it does on this machine; the commands it runs are real processes here. Share only with people you trust.",
  sharedTo: "Shared with",
  shareGo: "Share",
  shareAfter: "To give one agent and not the machine, press Lend… beside that login on the machines page.",
  lendTitle: (login, machine) => `Lend ${login} on ${machine} to another account`,
  lendWhat: "What is lent is this one login, not the machine: the other account can talk to it and ask it as a colleague, and it works for them on their own machines.",
  lendSees: "It lends that login's whole account, its subscription's usage with it; everything it does for them happens on this machine of yours, where you see it and can stop it. Lend only to people you trust.",
  lentTo: "Lent to",
  lendGo: "Lend it",
  nobodyYet: "Given to nobody yet.",
  to: "Their account",
  toHint: "Their account's name on this hub",
  takeBack: "Take back",
  relaysTitle: (machine) => `The relays ${machine} connects through`,
  relaysWhat: "A relay is a port that forwards only to the hub, such as fastctx relay on another machine with a better line. This machine tries them in order and goes straight to the hub when none answers; a relay cannot read what passes through it.",
  relayLabel: "Name",
  relayAddresses: "Addresses",
  relayAddressesHint: "IP and port, several separated by commas, such as 203.0.113.5:7443",
  addRelay: "Add a relay",
  removeRelay: "Remove this one",
  noRelays: "It connects straight to the hub.",
  hubLast: "When none answers, straight to the hub.",
  badAddress: (text) => `“${text}” is not an IP and port.`,
  needsLabel: "Each relay needs a name.",
  relaysGo: "Connect this way",
  removeTitle: (machine) => `Remove ${machine}`,
  removeWhat: (machine) =>
    `${machine} is taken off the hub: it no longer connects to it, every account it is logged in to there logs it out, and what it shared and lent ends. Nothing on it is deleted; to come back it logs in again.`,
  removeGo: "Remove it",
  reason: "Reason",
  reasonDefault: "removed",
  working: "Working…",
  close: "Close",
  failed: (why) => `Not done: ${why}`,
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
  hub: "中枢",
  account: "账号",
  level: { super: "超级管理员", member: "成员" },
  superNote: (admin) => `这个中枢的超级管理员是 ${admin}：你的每一台机器，${admin} 都看得见、用得了。`,
  thisOne: "这台",
  arrived: (kind, name, at) => `${at}，账号里进来了${kind} ${name}`,
  me: "是我",
  notMe: "不是我",
  notMeWhat: (name) => `${name} 不是你放进来的：移除它，它就不能再以这个账号的身份看、说话或干活。`,
  devices: "已登录的设备",
  devicesWhat: "每一台都带着自己的钥匙，以这个账号的身份进来。丢了的设备就在这里移除。",
  kinds: { computer: "电脑", phone: "手机", web: "网页上的 AI" },
  machineTag: "这个账号的机器",
  key: (start) => `钥匙 ${start}`,
  added: (at) => `${at} 登录`,
  onlineNow: "在线",
  lastSeen: (at) => `${at} 最后在线`,
  neverSeen: "还没连上过",
  removeDevice: "移除…",
  removeDeviceTitle: (name) => `移除 ${name}`,
  removeDeviceWhat: (name, account, machine) =>
    machine
      ? `${name} 是 ${account} 的机器。移除以后它从中枢上除名，不再连这个中枢，它在这里登录的账号都随之退出，它的共享与借出一并作废。它身上的东西不会被删掉。`
      : `${name} 退出 ${account}：它不能再以这个账号的身份看、说话或干活。它还登录着这个中枢上别的账号的话，那些照旧。`,
  removeDeviceGo: "移除",
  invite: "让别的设备进来",
  inviteWhat: "给出一个一次性的令牌：手机扫二维码，电脑粘贴它，服务器运行一整行命令。",
  invitePhone: "一部手机",
  inviteComputer: "一台电脑",
  inviteServer: "一台 Linux 服务器",
  reaching: "能访问你的机器的智能体",
  reachingWhat: "你用过别人机器上的智能体，那台机器上的智能体就能访问你的机器，那台机器的主人因此也能。不再用了，就在这里拿掉。",
  reachingNone: "还没有。你第一次用别人机器上的智能体时，那台机器会出现在这里。",
  reachingLine: (machine, owner) => (owner ? `${owner} 的 ${machine} 上的智能体能访问你的机器` : `${machine} 上的智能体能访问你的机器`),
  reachingSince: (at) => `${at} 起`,
  stopReaching: "拿掉…",
  stopReachingTitle: (machine) => `不让 ${machine} 上的智能体再访问你的机器`,
  stopReachingWhat: (machine, owner) =>
    `拿掉以后，${machine} 上替你干活的对话再去你的机器会被拒绝。你下次再用它上面的智能体，它又能访问你的机器。${owner ? `${owner} 那边` : "那台机器上"}什么都不会被删掉。`,
  stopReachingGo: "拿掉",
  projects: "项目",
  noProjects: "这个账号还没有项目。",
  sharedWith: "共享给",
  sharedBy: (owner) => `${owner} 共享给你的`,
  share: "共享…",
  shareTitle: (project) => `共享项目 ${project}`,
  shareWith: "共享给哪个账号",
  shareGo: "共享",
  unshare: (account) => `不再共享给 ${account}`,
  password: "口令",
  passwordWhat: "已经登录着的设备不受影响；以后新登录要用新口令。",
  changePassword: "改口令…",
  passwordTitle: "改口令",
  newPassword: "新口令",
  passwordAgain: "再输一遍新口令",
  passwordsDiffer: "两遍口令不一样。",
  passwordGo: "改成这个",
  passwordDone: "口令改好了。",
  logout: "退出这个账号…",
  logoutTitle: (login) => `从 ${login} 退出`,
  logoutWhat: (login, registered) =>
    registered
      ? `这台电脑从 ${login} 退出，也不再是这个账号的机器：这个账号的智能体不能再住在这里、在这里干活，它的共享与借出一并作废。这台电脑上的东西不会被删掉。`
      : `这台设备从 ${login} 退出，不能再以这个账号的身份看和说话。`,
  logoutGo: "退出",
  adminPage: "管理这个中枢…",
  another: "再登录一个账号",
  none: "还没有登录任何账号。",
  logIn: "登录…",
  working: "稍等…",
  close: "关闭",
  failed: (why) => `没办成：${why}`,
};

export const enAccount: AccountWords = {
  title: "Account",
  hub: "Hub",
  account: "Account",
  level: { super: "super administrator", member: "member" },
  superNote: (admin) => `${admin} is this hub's super administrator, and sees and may use every machine of yours.`,
  thisOne: "this one",
  arrived: (kind, name, at) => `${at}: the ${kind} ${name} came into the account`,
  me: "That was me",
  notMe: "That wasn't me",
  notMeWhat: (name) => `${name} was not let in by you: remove it, and it can no longer look, talk or work as this account.`,
  devices: "Devices logged in",
  devicesWhat: "Each carries its own key and comes in as this account. Remove a lost device here.",
  kinds: { computer: "computer", phone: "phone", web: "web AI" },
  machineTag: "a machine of this account",
  key: (start) => `key ${start}`,
  added: (at) => `logged in ${at}`,
  onlineNow: "online",
  lastSeen: (at) => `last online ${at}`,
  neverSeen: "never connected yet",
  removeDevice: "Remove…",
  removeDeviceTitle: (name) => `Remove ${name}`,
  removeDeviceWhat: (name, account, machine) =>
    machine
      ? `${name} is a machine of ${account}. Removed, it is taken off the hub and no longer connects to it; every account it is logged in to there logs it out, and what it shared and lent ends. Nothing on it is deleted.`
      : `${name} logs out of ${account}: it can no longer look, talk or work as this account. Other accounts it is logged in to on this hub stay as they are.`,
  removeDeviceGo: "Remove",
  invite: "Let another device in",
  inviteWhat: "A one-time token: a phone scans its QR code, a computer pastes it, a server runs one line.",
  invitePhone: "A phone",
  inviteComputer: "A computer",
  inviteServer: "A Linux server",
  reaching: "Agents that can reach your machines",
  reachingWhat: "Once you used an agent on another's machine, the agents on that machine can reach your machines, and so can its owner. Take it back here when you no longer use them.",
  reachingNone: "None yet. The first time you use an agent on another's machine, that machine shows here.",
  reachingLine: (machine, owner) => (owner ? `The agents on ${owner}'s ${machine} can reach your machines` : `The agents on ${machine} can reach your machines`),
  reachingSince: (at) => `since ${at}`,
  stopReaching: "Take back…",
  stopReachingTitle: (machine) => `Stop the agents on ${machine} reaching your machines`,
  stopReachingWhat: (machine, owner) =>
    `Then conversations on ${machine} working for you are refused when they go to your machines. The next time you use an agent there, it can reach them again. Nothing ${owner ? `of ${owner}'s` : "on that machine"} is deleted.`,
  stopReachingGo: "Take it back",
  projects: "Projects",
  noProjects: "This account has no projects yet.",
  sharedWith: "Shared with",
  sharedBy: (owner) => `shared with you by ${owner}`,
  share: "Share…",
  shareTitle: (project) => `Share the project ${project}`,
  shareWith: "Share with which account",
  shareGo: "Share",
  unshare: (account) => `Stop sharing with ${account}`,
  password: "Password",
  passwordWhat: "Devices logged in already stay so; new logins take the new one.",
  changePassword: "Change the password…",
  passwordTitle: "Change the password",
  newPassword: "New password",
  passwordAgain: "New password again",
  passwordsDiffer: "The two passwords differ.",
  passwordGo: "Change it",
  passwordDone: "The password is changed.",
  logout: "Log out of this account…",
  logoutTitle: (login) => `Log out of ${login}`,
  logoutWhat: (login, registered) =>
    registered
      ? `This computer logs out of ${login} and is no longer one of its machines: the account's agents can no longer live or work here, and what it shared and lent ends. Nothing on this computer is deleted.`
      : `This device logs out of ${login} and can no longer look or talk as this account.`,
  logoutGo: "Log out",
  adminPage: "Manage this hub…",
  another: "Log in to another account",
  none: "Logged in to no account yet.",
  logIn: "Log in…",
  working: "Working…",
  close: "Close",
  failed: (why) => `Not done: ${why}`,
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
  title: "管理中枢",
  hub: (name) => `中枢 ${name}`,
  superPart: "超级管理员",
  superWhat: "每一台机器你都能用；这个身份只有一个，可以交给别的账号。",
  transfer: "交给另一个账号…",
  transferTitle: "把超级管理员交给另一个账号",
  transferWhat: "交出去之后你就是成员：只能用自己的机器，和别人共享、借给你的东西，这一页也不再归你。对方从此能用这个中枢上的每一台机器。",
  transferTo: "交给",
  transferGo: "交出去",
  noOther: "这个中枢上还没有别的账号可以交。",
  selfRegistration: "允许自己注册",
  selfRegistrationWhat: "打开以后，任何够得着这个中枢的人都能自己开一个账号；自己注册来的都是成员。",
  accounts: "账号",
  opened: (at) => `${at} 开的`,
  superTag: "超级管理员",
  locked: "已锁定",
  you: "你",
  lock: "锁定",
  unlock: "解锁",
  resetPassword: "重设口令…",
  resetTitle: (account) => `重设 ${account} 的口令`,
  resetWhat: "已经登录着的设备不受影响；以后新登录要用新口令。把新口令当面告诉对方。",
  resetGo: "重设",
  deleteAccount: "删除…",
  deleteTitle: (account) => `删除账号 ${account}`,
  deleteWhat: (account, machines, projects) =>
    [
      `删除 ${account} 不能撤回。`,
      machines.length ? `它名下的机器 ${machines.join("、")} 随之移除，不再连这个中枢。` : "它名下没有机器。",
      "它给出与收到的共享、借出一并作废。",
      projects.length ? `项目 ${projects.join("、")} 的账本挪进归档，一个字不删，可以交给另一个账号。` : "它没有项目。",
    ].join(""),
  deleteGo: "删除",
  openAccount: "开一个账号…",
  openTitle: "开一个账号",
  openWhat: "开出来的账号都是成员：能登记自己的电脑、把机器共享出去、把智能体借出去、开项目。",
  name: "账号名",
  nameHint: "小写字母、数字与连字符，以字母开头",
  password: "口令",
  passwordAgain: "再输一遍口令",
  passwordsDiffer: "两遍口令不一样。",
  openGo: "开账号",
  projects: "项目",
  owner: "属于",
  handTo: "交给…",
  handTitle: (project) => `把项目 ${project} 交给另一个账号`,
  handWhat: "项目连同账本整个归到那个账号名下；它横跨的机器若不是那个账号能干活的，那个账号的智能体就进不去。",
  handGo: "交出去",
  notAdmin: "这台设备登录的账号都不是哪个中枢的超级管理员。",
  working: "稍等…",
  close: "关闭",
  failed: (why) => `没办成：${why}`,
};

export const enAdmin: AdminWords = {
  title: "Manage the hub",
  hub: (name) => `Hub ${name}`,
  superPart: "Super administrator",
  superWhat: "You may use every machine on it; there is one, and you can hand it on.",
  transfer: "Hand it to another account…",
  transferTitle: "Hand being super administrator to another account",
  transferWhat: "Once handed on, you are a member: you use your own machines and what others share and lend you, and this page is no longer yours. The other account may use every machine on this hub from then on.",
  transferTo: "Hand to",
  transferGo: "Hand it on",
  noOther: "This hub has no other account to hand it to yet.",
  selfRegistration: "Anyone may open an account",
  selfRegistrationWhat: "Then anyone who reaches this hub can open an account of their own; each is a member.",
  accounts: "Accounts",
  opened: (at) => `opened ${at}`,
  superTag: "super administrator",
  locked: "locked",
  you: "you",
  lock: "Lock",
  unlock: "Unlock",
  resetPassword: "Reset the password…",
  resetTitle: (account) => `Reset the password of ${account}`,
  resetWhat: "Devices logged in already stay so; new logins take the new password. Tell it to them in person.",
  resetGo: "Reset it",
  deleteAccount: "Delete…",
  deleteTitle: (account) => `Delete the account ${account}`,
  deleteWhat: (account, machines, projects) =>
    [
      `Deleting ${account} cannot be undone. `,
      machines.length ? `Its machines ${machines.join(", ")} are taken off the hub with it. ` : "No machine is registered under it. ",
      "What it shared and lent, and what was shared and lent to it, ends. ",
      projects.length ? `The projects ${projects.join(", ")} go to the archive whole, not a word lost, and can be handed to another account.` : "It has no projects.",
    ].join(""),
  deleteGo: "Delete",
  openAccount: "Open an account…",
  openTitle: "Open an account",
  openWhat: "Every account opened is a member: it registers its own computers, shares its machines, lends its agents and opens projects.",
  name: "Account name",
  nameHint: "lowercase letters, digits and hyphens, starting with a letter",
  password: "Password",
  passwordAgain: "Password again",
  passwordsDiffer: "The two passwords differ.",
  openGo: "Open it",
  projects: "Projects",
  owner: "Belongs to",
  handTo: "Hand to…",
  handTitle: (project) => `Hand ${project} to another account`,
  handWhat: "The project, ledger and all, goes to that account. Where it spans machines that account cannot work on, that account's agents cannot go in.",
  handGo: "Hand it over",
  notAdmin: "No account this device is logged in to is a hub's super administrator.",
  working: "Working…",
  close: "Close",
  failed: (why) => `Not done: ${why}`,
};
