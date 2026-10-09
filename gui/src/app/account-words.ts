// What the pages about accounts say (`world-gui.md`, 账号、机器与授权):
// logging in; the machines a login may use, and what is set on them; the
// account itself, with its devices, who came into it, the one-time tokens
// that let another device in, its password and its projects; and an
// administrator's page of accounts. In the user's words (`world-gui.md`, 用词).

import type { MemberKind } from "../world/accounts";

export interface LoginWords {
  lede: string;
  thisComputer: string;
  thisPhone: string;
  facts(name: string): string;
  placed: string;
  notPlaced: string;
  place: string;
  noCarried: string;
  serviceOn: string;
  serviceOff: string;
  /** The logins this computer or device has already. */
  already(logins: string): string;
  /** Between the items of a list. */
  separator: string;
  title: string;
  another: string;
  hub: string;
  hubHint: string;
  account: string;
  password: string;
  passwordAgain: string;
  passwordsDiffer: string;
  register: string;
  registerWhat: string;
  name: string;
  phoneName: string;
  nameHint: string;
  go: string;
  going: string;
  withToken: string;
  withPassword: string;
  openOne: string;
  openTitle: string;
  openWhat: string;
  openGo: string;
  token: string;
  tokenHint: string;
  /** On a phone: read a token's QR code with the camera. */
  scan: string;
  scanFailed(why: string): string;
  tokenOr: string;
  alone: string;
  aloneWhat: string;
  /** Logged in: `hub · account`, and this computer's name. */
  done(login: string, name: string): string;
  registered: string;
  onlyLoggedIn: string;
  phoneDone: string;
  kept: Record<"already" | "installed" | "declined" | "unsupported", string>;
  keptCommand: string;
  hosts: string;
  hostsWhat: string;
  codex: string;
  claude: string;
  hostState: Record<"absent" | "out" | "in" | "old", string>;
  applyGo: string;
  applying: string;
  enter: string;
  back: string;
  failed(why: string): string;
}

export const zhLogin: LoginWords = {
  lede: "让你的几台电脑，和住在上面的 Codex、Claude，在同一个世界里。",
  thisComputer: "这台电脑",
  thisPhone: "这部手机",
  facts: (name) => `叫 ${name}`,
  placed: "fastctx 已就位",
  notPlaced: "fastctx 还没放好",
  place: "放好它",
  noCarried: "这一份 World 没有随身带 fastctx",
  serviceOn: "开机自启的服务在",
  serviceOff: "还没有开机自启的服务",
  already: (logins) => `已经登录着 ${logins}。可以再登录一个，别的中枢的、同一个中枢里别的账号的都行。`,
  separator: "、",
  title: "登录",
  another: "再登录一个账号",
  hub: "服务器",
  hubHint: "中枢的地址加端口，比如 hub.example.com:7443",
  account: "账号",
  password: "口令",
  passwordAgain: "再输一遍口令",
  passwordsDiffer: "两遍口令不一样。",
  register: "把这台电脑登记成这个账号的机器",
  registerWhat: "登记以后，这个账号的智能体能住在这台电脑上、在上面干活。不勾就只登录：从这里看、和智能体说话，这台电脑不归这个账号用。",
  name: "这台电脑叫什么",
  phoneName: "这部手机叫什么",
  nameHint: "小写字母、数字与连字符；一台设备在每个中枢上都叫同一个名字",
  go: "登录",
  going: "正在登录…",
  withToken: "用一次性令牌登录",
  withPassword: "用口令登录",
  openOne: "没有账号？在这个中枢上开一个",
  openTitle: "开一个账号",
  openWhat: "只有中枢允许自己注册时才开得成；不允许的话，请它的管理员替你开。",
  openGo: "开账号并登录",
  token: "令牌",
  tokenHint: "另一台登录着的设备给你的、以 FX5 开头的一长串",
  scan: "扫电脑上的二维码",
  scanFailed: (why) => `没扫成：${why}`,
  tokenOr: "或者粘贴令牌",
  alone: "先不登录，只用这台电脑",
  aloneWhat: "这台电脑本身就是一个世界，它自己的智能体、项目都照常。以后随时可以登录。",
  done: (login, name) => `登录好了：${login}，这台叫 ${name}。`,
  registered: "它登记成了这个账号的机器。",
  onlyLoggedIn: "它只登录着，没有登记成这个账号的机器。",
  phoneDone: "这部手机现在带着自己的钥匙，以这个账号的身份看世界、和智能体说话；上面什么都不跑。",
  kept: {
    already: "开机自启的服务早就在了。",
    installed: "开机自启的服务装好了：从此开机就连着，不管有没有人登录。",
    declined: "没有拿到管理员确认，所以没装开机服务：只有你登录着系统时它才连着。以后可以在设置里再装。",
    unsupported: "这个系统没有开机服务：只有你登录着系统时它才连着。",
  },
  keptCommand: "要它开机就连着，在终端里运行：",
  hosts: "把世界的工具接进这台电脑上的智能体",
  hostsWhat: "接进去之后，你在这台电脑上自己开的 Codex 与 Claude Code 也能用世界的手、看见世界。随时可以拿出来。",
  codex: "Codex",
  claude: "Claude Code",
  hostState: { absent: "没装", out: "没接入", in: "已接入", old: "里面是旧版 fastctx" },
  applyGo: "接入",
  applying: "正在接入…",
  enter: "进入世界",
  back: "返回",
  failed: (why) => `没办成：${why}`,
};

export const enLogin: LoginWords = {
  lede: "Your computers, and the Codex and Claude that live on them, in one World.",
  thisComputer: "This computer",
  thisPhone: "This phone",
  facts: (name) => `named ${name}`,
  placed: "fastctx is in place",
  notPlaced: "fastctx is not in place yet",
  place: "Put it in place",
  noCarried: "this World app carries no fastctx",
  serviceOn: "the startup service is on",
  serviceOff: "no startup service yet",
  already: (logins) => `Logged in to ${logins}. You can log in to another one too, on another hub or on the same.`,
  separator: ", ",
  title: "Log in",
  another: "Log in to another account",
  hub: "Server",
  hubHint: "The hub's address and port, such as hub.example.com:7443",
  account: "Account",
  password: "Password",
  passwordAgain: "Password again",
  passwordsDiffer: "The two passwords differ.",
  register: "Register this computer as one of the account's machines",
  registerWhat: "Registered, the account's agents can live on this computer and work on it. Unticked, it only logs in: you look and talk from here, and the account does not use this computer.",
  name: "This computer's name",
  phoneName: "This phone's name",
  nameHint: "lowercase letters, digits and hyphens; a device has the same name on every hub",
  go: "Log in",
  going: "Logging in…",
  withToken: "Log in with a one-time token",
  withPassword: "Log in with a password",
  openOne: "No account? Open one on this hub",
  openTitle: "Open an account",
  openWhat: "Only a hub that lets anyone open an account will; otherwise ask its administrator to open one for you.",
  openGo: "Open it and log in",
  token: "Token",
  tokenHint: "The long string beginning FX5 that another logged-in device gave you",
  scan: "Scan the code on the computer",
  scanFailed: (why) => `Could not scan it: ${why}`,
  tokenOr: "or paste the token",
  alone: "Not now: use this computer on its own",
  aloneWhat: "This computer is a World by itself: its agents and projects work as ever. Log in whenever you like.",
  done: (login, name) => `Logged in to ${login}; this one is ${name}.`,
  registered: "It is registered as one of the account's machines.",
  onlyLoggedIn: "It is logged in only, not registered as one of the account's machines.",
  phoneDone: "This phone now carries its own key: it looks at the World and talks to agents as this account. Nothing runs on it.",
  kept: {
    already: "The startup service was already there.",
    installed: "The startup service is in: from now on it is connected from boot, whether or not anyone is signed in.",
    declined: "No administrator approval, so no startup service: it is connected only while you are signed in. You can add it later in the settings.",
    unsupported: "This system has no startup service: it is connected only while you are signed in.",
  },
  keptCommand: "To keep it connected from boot, run in a terminal:",
  hosts: "Put the World's tools into this computer's agents",
  hostsWhat: "Then the Codex and Claude Code you open on this computer yourself can use the World's hands and see the World too. You can take them out any time.",
  codex: "Codex",
  claude: "Claude Code",
  hostState: { absent: "not installed", out: "not in", in: "in", old: "has the old fastctx" },
  applyGo: "Put them in",
  applying: "Putting them in…",
  enter: "Enter the World",
  back: "Back",
  failed: (why) => `Not done: ${why}`,
};

export interface MachinesWords {
  title: string;
  /** The head's button that lets another machine or device in. */
  add: string;
  here: string;
  /** A login, heading the machines it may use. */
  group(login: string): string;
  online(since: string): string;
  onlineNow: string;
  offline(since: string | null): string;
  unknown: string;
  factsUnknown(why: string): string;
  /** How this computer reaches a hub. */
  link(hub: string, state: string): string;
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
  /** What another account let this one do on a machine. */
  granted(owner: string, rights: string): string;
  /** Whom this account let use its machine. */
  grantsOut(list: string): string;
  /** One account it lent the machine to, and for what. */
  lent(account: string, rights: string): string;
  separator: string;
  hands: string;
  agentsRight: string;
  and: string;
  trusted: string;
  agents: string;
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
  grant: string;
  relays: string;
  remove: string;
  trust: string;
  untrust: string;
  /** The logins see no machine but this computer. */
  none: string;
  noneDevice: string;
  /** On its own, logged in nowhere. */
  alone: string;
  logIn: string;
}

export const zhMachines: MachinesWords = {
  title: "机器",
  add: "添加",
  here: "这台",
  group: (login) => login,
  online: (since) => `在线，${since} 起`,
  onlineNow: "在线",
  offline: (since) => (since ? `离线，${since} 起` : "离线"),
  unknown: "不知道在不在线",
  factsUnknown: (why) => `问不到它的情况：${why}`,
  link: (hub, state) => `中枢 ${hub}：${state}`,
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
  granted: (owner, rights) => `${owner} 授权你${rights}；你在上面做的事、留在那里的输出，${owner} 都看得见`,
  grantsOut: (list) => `也授权给了 ${list}`,
  lent: (account, rights) => `${account}（${rights}）`,
  separator: "、",
  hands: "在上面干活",
  agentsRight: "用上面的智能体",
  and: "、",
  trusted: "信任",
  agents: "上面的智能体",
  nobody: "此刻没有智能体",
  jobs: (n) => `${n} 个作业在跑`,
  home: "家目录",
  hosts: "上面的两家智能体",
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
  grant: "授权…",
  relays: "转接站…",
  remove: "移除…",
  trust: "信任…",
  untrust: "不再信任",
  none: "这个账号此刻只能用这台电脑。在别的电脑上登录同一个账号并登记，或者让别的账号授权给你，它们就出现在这里。",
  noneDevice: "这个账号此刻没有能用的机器。在一台电脑上登录同一个账号并登记，它就出现在这里。",
  alone: "这台电脑还没有登录任何账号：它独自一台，只有它自己。",
  logIn: "登录…",
};

export const enMachines: MachinesWords = {
  title: "Machines",
  add: "Add",
  here: "this one",
  group: (login) => login,
  online: (since) => `online since ${since}`,
  onlineNow: "online",
  offline: (since) => (since ? `offline since ${since}` : "offline"),
  unknown: "not known whether online",
  factsUnknown: (why) => `could not be asked: ${why}`,
  link: (hub, state) => `Hub ${hub}: ${state}`,
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
  granted: (owner, rights) => `${owner} lets you ${rights}, and sees what you do there and the output you leave`,
  grantsOut: (list) => `also lent to ${list}`,
  lent: (account, rights) => `${account} (to ${rights})`,
  separator: "; ",
  hands: "work on it",
  agentsRight: "use its agents",
  and: " and ",
  trusted: "trusted",
  agents: "Agents on it",
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
  grant: "Lend…",
  relays: "Relays…",
  remove: "Remove…",
  trust: "Trust…",
  untrust: "Stop trusting",
  none: "This account can use only this computer for now. Log in to it on another computer and register it, or have another account lend you one, and it shows here.",
  noneDevice: "This account has no machine to use yet. Log in to it on a computer and register it, and it shows here.",
  alone: "This computer is logged in to no account: it is on its own, a World of one.",
  logIn: "Log in…",
};

/** The sheets on a machine: whom its owner lends it to, whether to trust
 *  one lent, the relays it connects through, and removing it. */
export interface AccessWords {
  grantTitle(machine: string): string;
  grantWhat: string;
  grantSees: string;
  lentTo: string;
  nobodyYet: string;
  to: string;
  toHint: string;
  hands: string;
  handsWhat: string;
  agents: string;
  agentsWhat: string;
  grantGo: string;
  change: string;
  takeBack: string;
  trustTitle(machine: string): string;
  trustWhat(owner: string): string;
  trustGo: string;
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
  grantTitle: (machine) => `把 ${machine} 授权给别的账号`,
  grantWhat: "同一个中枢上的别的账号，可以在你这台机器上干活，或者用上面的智能体，或者两样都行。",
  grantSees: "对方在这台机器上做的一切你都看得见；它跑的命令是这台机器上真实的进程。",
  lentTo: "已经授权给",
  nobodyYet: "还没授权给谁。",
  to: "对方的账号",
  toHint: "对方在这个中枢上的账号名",
  hands: "在上面干活",
  handsWhat: "在这台机器上跑命令、读写文件",
  agents: "用上面的智能体",
  agentsWhat: "和住在这台机器上的智能体说话，在这里开新的对话",
  grantGo: "授权",
  change: "改好",
  takeBack: "收回",
  trustTitle: (machine) => `信任 ${machine}`,
  trustWhat: (owner) =>
    `在它上面替你干活的会话，能去你的其它机器，就像你自己的电脑一样；你在它上面做的一切，它的主人 ${owner} 看得见。只信任你信得过它主人的机器。`,
  trustGo: "信任它",
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
    `${machine} 从中枢上除名：不再连这个中枢，它在这里登录的账号都随之退出，授权与信任一并作废。它身上的东西不会被删掉；要回来，得在它上面重新登录。`,
  removeGo: "移除它",
  reason: "原因",
  reasonDefault: "removed",
  working: "稍等…",
  close: "关闭",
  failed: (why) => `没办成：${why}`,
};

export const enAccess: AccessWords = {
  grantTitle: (machine) => `Lend ${machine} to another account`,
  grantWhat: "Another account on the same hub can work on this machine of yours, use the agents on it, or both.",
  grantSees: "You see everything it does on this machine; the commands it runs are real processes here.",
  lentTo: "Lent to",
  nobodyYet: "Lent to nobody yet.",
  to: "Their account",
  toHint: "Their account's name on this hub",
  hands: "Work on it",
  handsWhat: "run commands and read and write files on this machine",
  agents: "Use its agents",
  agentsWhat: "talk to the agents that live on this machine, and open new conversations here",
  grantGo: "Lend it",
  change: "Change",
  takeBack: "Take back",
  trustTitle: (machine) => `Trust ${machine}`,
  trustWhat: (owner) =>
    `Conversations working for you on it can go to your other machines, as from a computer of your own; ${owner}, who owns it, sees everything you do on it. Trust only machines whose owner you trust.`,
  trustGo: "Trust it",
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
    `${machine} is taken off the hub: it no longer connects to it, every account it is logged in to there logs it out, and what was lent to it or trusted in it ends. Nothing on it is deleted; to come back it logs in again.`,
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
  admin: string;
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
  admin: "管理员",
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
      ? `${name} 是 ${account} 的机器。移除以后它从中枢上除名，不再连这个中枢，它在这里登录的账号都随之退出，授权与信任一并作废。它身上的东西不会被删掉。`
      : `${name} 退出 ${account}：它不能再以这个账号的身份看、说话或干活。它还登录着这个中枢上别的账号的话，那些照旧。`,
  removeDeviceGo: "移除",
  invite: "让别的设备进来",
  inviteWhat: "给出一个一次性的令牌：手机扫二维码，电脑粘贴它，服务器运行一整行命令。",
  invitePhone: "一部手机",
  inviteComputer: "一台电脑",
  inviteServer: "一台 Linux 服务器",
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
      ? `这台电脑从 ${login} 退出，也不再是这个账号的机器：这个账号的智能体不能再住在这里、在这里干活，授权与信任一并作废。这台电脑上的东西不会被删掉。`
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
  admin: "administrator",
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
      ? `${name} is a machine of ${account}. Removed, it is taken off the hub and no longer connects to it; every account it is logged in to there logs it out, and what was lent to it or trusted in it ends. Nothing on it is deleted.`
      : `${name} logs out of ${account}: it can no longer look, talk or work as this account. Other accounts it is logged in to on this hub stay as they are.`,
  removeDeviceGo: "Remove",
  invite: "Let another device in",
  inviteWhat: "A one-time token: a phone scans its QR code, a computer pastes it, a server runs one line.",
  invitePhone: "A phone",
  inviteComputer: "A computer",
  inviteServer: "A Linux server",
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
      ? `This computer logs out of ${login} and is no longer one of its machines: the account's agents can no longer live or work here, and what was lent to it or trusted in it ends. Nothing on this computer is deleted.`
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

/** The sheet that makes a one-time token for another device. */
export interface TokenWords {
  title(login: string): string;
  what: string;
  kinds: Record<"phone" | "computer" | "server", string>;
  hints: Record<"phone" | "computer" | "server", string>;
  register: string;
  registerWhat: string;
  mayNotRegister: string;
  name: string;
  nameHint: string;
  valid: string;
  hours(n: number): string;
  make: string;
  making: string;
  qr: string;
  token: string;
  command: string;
  copy: string;
  copied: string;
  until(at: string): string;
  steps: Record<"phone" | "computer" | "server", string[]>;
  howTo: string;
  again: string;
  close: string;
  failed(why: string): string;
}

export const zhToken: TokenWords = {
  title: (login) => `让一台设备进入 ${login}`,
  what: "它是",
  kinds: { phone: "一部手机", computer: "一台电脑", server: "一台 Linux 服务器" },
  hints: {
    phone: "手机扫二维码登录这个账号，只用来看和说话，上面不跑任何东西。",
    computer: "在那台电脑上的 World 里选「用一次性令牌登录」，粘贴令牌。",
    server: "在服务器的终端里运行一整行命令：它装好 fastctx，登记成这个账号的机器，再装开机服务。",
  },
  register: "登记成这个账号的机器",
  registerWhat: "这个账号的智能体就能住在它上面、在上面干活；不勾就只登录。",
  mayNotRegister: "这个账号不能登记机器，问中枢的管理员。",
  name: "它叫什么",
  nameHint: "小写字母、数字与连字符；不写就由它自己定",
  valid: "多久内有效",
  hours: (n) => (n < 24 ? `${n} 小时` : `${n / 24} 天`),
  make: "生成令牌",
  making: "正在生成…",
  qr: "令牌的二维码，用手机上的 World 扫它",
  token: "令牌",
  command: "在服务器上运行",
  copy: "复制",
  copied: "已复制",
  until: (at) => `${at} 之前有效，只能用一次`,
  steps: {
    phone: [
      "在手机上装好 World，打开它，选「用一次性令牌登录」，扫这个码；扫不了就粘贴令牌。",
      "它以这个账号的身份进来：能看、能说话，但不在它上面跑任何东西。",
      "要它在后台也连着，按它的提示在手机的设置里放行后台运行。",
    ],
    computer: [
      "在那台电脑上装好 World，打开它，选「用一次性令牌登录」，粘贴令牌；",
      "或者在它的终端里运行 fastctx login --token <令牌>。",
      "登录时会请一次管理员确认，装上开机自启的服务，之后它就一直连着。",
    ],
    server: [
      "把这一整行粘进 Linux 服务器的终端运行；",
      "它下载 fastctx、核对摘要无误后才解包，凭令牌登录并登记成这个账号的机器；",
      "装开机服务那一步要管理员权限，不是 root 时它会说明怎样用 sudo。中途断了，同一行再跑一次就接着做完。",
    ],
  },
  howTo: "在那台设备上",
  again: "再生成一个",
  close: "关闭",
  failed: (why) => `没办成：${why}`,
};

export const enToken: TokenWords = {
  title: (login) => `Let a device into ${login}`,
  what: "It is",
  kinds: { phone: "A phone", computer: "A computer", server: "A Linux server" },
  hints: {
    phone: "The phone scans the QR code and logs in to this account, to look and talk only; nothing runs on it.",
    computer: "In World on that computer, choose “Log in with a one-time token” and paste the token.",
    server: "Run one line in the server's terminal: it installs fastctx, registers it as one of this account's machines, and installs the startup service.",
  },
  register: "Register it as one of this account's machines",
  registerWhat: "Then the account's agents can live and work on it; unticked, it only logs in.",
  mayNotRegister: "This account may not register machines; ask the hub's administrator.",
  name: "Its name",
  nameHint: "lowercase letters, digits and hyphens; left empty, it names itself",
  valid: "Valid for",
  hours: (n) => (n < 24 ? `${n} hour${n === 1 ? "" : "s"}` : `${n / 24} day${n === 24 ? "" : "s"}`),
  make: "Make a token",
  making: "Making it…",
  qr: "The token's QR code: scan it with World on the phone",
  token: "Token",
  command: "Run on the server",
  copy: "Copy",
  copied: "Copied",
  until: (at) => `valid until ${at}, once`,
  steps: {
    phone: [
      "Install World on the phone, open it, choose “Log in with a one-time token”, and scan this code; or paste the token.",
      "It comes in as this account: it can look and talk, and nothing runs on it.",
      "To keep it connected in the background, allow it in the phone's settings as it asks.",
    ],
    computer: [
      "Install World on that computer, open it, choose “Log in with a one-time token”, and paste the token;",
      "or run fastctx login --token <token> in its terminal.",
      "Logging in asks an administrator once to install the startup service; after that it stays connected.",
    ],
    server: [
      "Paste this whole line into the Linux server's terminal and run it;",
      "it downloads fastctx, unpacks it only once the digest checks out, logs in with the token and registers as one of this account's machines;",
      "installing the startup service needs administrator rights; when not root it says how to use sudo. If it is cut off, run the same line again and it finishes.",
    ],
  },
  howTo: "On that device",
  again: "Make another",
  close: "Close",
  failed: (why) => `Not done: ${why}`,
};

export interface AdminWords {
  title: string;
  hub(name: string): string;
  selfRegistration: string;
  selfRegistrationWhat: string;
  accounts: string;
  opened(at: string): string;
  admin: string;
  mayRegister: string;
  locked: string;
  you: string;
  makeAdmin: string;
  unmakeAdmin: string;
  allowRegister: string;
  forbidRegister: string;
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
  name: string;
  nameHint: string;
  password: string;
  passwordAgain: string;
  passwordsDiffer: string;
  asAdmin: string;
  asAdminWhat: string;
  mayRegisterWhat: string;
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
  selfRegistration: "允许自己注册",
  selfRegistrationWhat: "打开以后，任何够得着这个中枢的人都能自己开一个账号；新账号默认不是管理员。",
  accounts: "账号",
  opened: (at) => `${at} 开的`,
  admin: "管理员",
  mayRegister: "能登记机器",
  locked: "已锁定",
  you: "你",
  makeAdmin: "设为管理员",
  unmakeAdmin: "取消管理员",
  allowRegister: "允许登记机器",
  forbidRegister: "不许登记机器",
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
      "它授出去的与收到的授权一并作废。",
      projects.length ? `项目 ${projects.join("、")} 的账本挪进归档，一个字不删，可以交给另一个账号。` : "它没有项目。",
    ].join(""),
  deleteGo: "删除",
  openAccount: "开一个账号…",
  openTitle: "开一个账号",
  name: "账号名",
  nameHint: "小写字母、数字与连字符，以字母开头",
  password: "口令",
  passwordAgain: "再输一遍口令",
  passwordsDiffer: "两遍口令不一样。",
  asAdmin: "管理员",
  asAdminWhat: "能管这一页的一切：开、锁、删账号，重设口令",
  mayRegisterWhat: "电脑登录它时能登记成它的机器",
  openGo: "开账号",
  projects: "项目",
  owner: "属于",
  handTo: "交给…",
  handTitle: (project) => `把项目 ${project} 交给另一个账号`,
  handWhat: "项目连同账本整个归到那个账号名下；它横跨的机器若不是那个账号能干活的，那个账号的智能体就进不去。",
  handGo: "交出去",
  notAdmin: "这台设备登录的账号都不是管理员。",
  working: "稍等…",
  close: "关闭",
  failed: (why) => `没办成：${why}`,
};

export const enAdmin: AdminWords = {
  title: "Manage the hub",
  hub: (name) => `Hub ${name}`,
  selfRegistration: "Anyone may open an account",
  selfRegistrationWhat: "Then anyone who reaches this hub can open an account of their own; a new account is never an administrator.",
  accounts: "Accounts",
  opened: (at) => `opened ${at}`,
  admin: "administrator",
  mayRegister: "may register machines",
  locked: "locked",
  you: "you",
  makeAdmin: "Make administrator",
  unmakeAdmin: "No longer administrator",
  allowRegister: "Let it register machines",
  forbidRegister: "Stop it registering machines",
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
      "What it lent and was lent ends. ",
      projects.length ? `The projects ${projects.join(", ")} go to the archive whole, not a word lost, and can be handed to another account.` : "It has no projects.",
    ].join(""),
  deleteGo: "Delete",
  openAccount: "Open an account…",
  openTitle: "Open an account",
  name: "Account name",
  nameHint: "lowercase letters, digits and hyphens, starting with a letter",
  password: "Password",
  passwordAgain: "Password again",
  passwordsDiffer: "The two passwords differ.",
  asAdmin: "Administrator",
  asAdminWhat: "manages everything on this page: opens, locks and deletes accounts, resets passwords",
  mayRegisterWhat: "a computer logging in to it may register as its machine",
  openGo: "Open it",
  projects: "Projects",
  owner: "Belongs to",
  handTo: "Hand to…",
  handTitle: (project) => `Hand ${project} to another account`,
  handWhat: "The project, ledger and all, goes to that account. Where it spans machines that account cannot work on, that account's agents cannot go in.",
  handGo: "Hand it over",
  notAdmin: "No account this device is logged in to is an administrator.",
  working: "Working…",
  close: "Close",
  failed: (why) => `Not done: ${why}`,
};
