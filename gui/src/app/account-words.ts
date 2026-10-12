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
import type { MachineLogAction } from "../world/machine-log";
import type { NetworkWords } from "../film/network";
import { agentName } from "../world/names";
import { enNameSaid, zhNameSaid, type NameSaid } from "./name-words";
import type { WhenWords } from "./when";
import { enMoment, enWhen, zhMoment, zhWhen, type MomentWords } from "./when-words";

export interface LoginWords {
  lede: string;
  title: string;
  /** The two ways in for a computer in no World yet. */
  join: string;
  joinWhat: string;
  create: string;
  createWhat: string;
  /** Making a World on a server over SSH (`remote-world.ts`). */
  remote: string;
  remoteWhat: string;
  server: string;
  sshPort: string;
  sshUser: string;
  signInBy: string;
  keyFile: string;
  keyPassword: string;
  keyMissing: string;
  /** The button that opens the system's file picker for the key. */
  chooseKey: string;
  /** After a field's label, for one that may be left empty. */
  optional: string;
  /** The bar a new password must clear, said before it is typed. */
  passwordFloor: string;
  next: string;
  firstTime: string;
  fingerprintWhat: string;
  trust: string;
  fingerprintChanged: string;
  sudoPassword: string;
  sudoWanted: string;
  portClosed(port: string): string;
  /** What the server said, in the person's words. */
  sshRefused: string;
  sshUnreachable: string;
  sudoWrong: string;
  notLinux: string;
  /** The server's architecture is neither x86_64 nor aarch64. */
  notArch: string;
  /** Adding a server to an existing World over SSH. */
  addGo: string;
  adding: string;
  remoteSteps: Record<"connect" | "check" | "upload" | "hub" | "login" | "join" | "arrive", string>;
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
  /** What is wrong with a machine or server name as typed. */
  nameSaid: NameSaid;
  /** `said` about the field labelled `field`, where a form has two names. */
  inField(field: string, said: string): string;
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
  /** What the hub is, in one line under the ways in. */
  hubIs: string;
  /** Under the title of making a World: what it asks of the machine the
   *  hub goes on. */
  createHub: string;
  remoteHub: string;
  /** The name a server takes in the World: its host name to begin with,
   *  read from it once signed in. */
  serverName: string;
  serverNameReading: string;
  /** Creating a World with this computer as its hub. */
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
// The bar a new password must clear, and how strong one is, wherever one is set.
const zhPasswordFloor = "至少 8 位，字母、数字、符号中至少两种";
const enPasswordFloor = "At least 8 characters, with two of letters, numbers and symbols";
const zhStrength: [string, string, string] = ["强度一般", "强度较高", "强度高"];
const enStrength: [string, string, string] = ["Fair", "Good", "Strong"];

export const zhLogin: LoginWords = {
  lede: "你的几台电脑，和电脑上的 Codex、Claude，同在一个世界。",
  title: "登录",
  join: "加入网络",
  joinWhat: "用中枢服务器地址和账号登录",
  create: "在这台电脑上建",
  createWhat: "这台电脑当中枢服务器",
  remote: "建到服务器上",
  remoteWhat: "用 SSH 让一台服务器当中枢服务器",
  hubIs: "中枢服务器认出网络里的每台设备，管着谁能用哪台机器，机器之间的话经它转达。",
  createHub: "这台电脑将是中枢服务器，要一直开着；它睡眠或关机时，别的设备都连不上。",
  remoteHub: "这台服务器将是中枢服务器，要一直开着；它停下时，别的设备都连不上。",
  server: "服务器地址",
  sshPort: "SSH 端口",
  sshUser: "用户名",
  signInBy: "登录方式",
  keyFile: "私钥文件",
  keyPassword: "私钥密码",
  keyMissing: "选一个私钥文件",
  chooseKey: "选择文件",
  optional: "（可选）",
  passwordFloor: zhPasswordFloor,
  next: "下一步",
  firstTime: "第一次连这台服务器",
  fingerprintWhat: "服务器的指纹",
  trust: "信任并继续",
  fingerprintChanged: "这台服务器的指纹和这台电脑记下的不一样，没有连接",
  sudoPassword: "sudo 密码",
  sudoWanted: "需要 sudo 密码",
  portClosed: (port) => `连不上端口 ${port}，防火墙或安全组要放行它`,
  sshRefused: "服务器没有接受这个用户名和密码或钥匙",
  sshUnreachable: "连不上这台服务器的 SSH 端口",
  sudoWrong: "sudo 密码不对",
  notLinux: "这台服务器不是 Linux",
  notArch: "这台服务器的架构不受支持，只支持 x86_64 与 aarch64",
  addGo: "添加",
  adding: "正在添加…",
  remoteSteps: { connect: "连接服务器", check: "查看服务器", upload: "上传程序", hub: "建网络", login: "这台电脑登录", join: "服务器加入", arrive: "中枢服务器搬过去" },
  another: "登录其他账号",
  hub: "中枢服务器地址",
  account: "账号",
  password: "密码",
  passwordAgain: "确认密码",
  passwordsDiffer: "两次输入的密码不一致",
  passwordRule: zhPasswordRule,
  strength: zhStrength,
  name: "电脑名称",
  phoneName: "手机名称",
  nameSaid: zhNameSaid,
  // The field takes the place of 名字 in the sentence: 电脑名称里不能有空格.
  inField: (field, said) => (said.includes("名字") ? said.replace("名字", field) : `${field}：${said}`),
  go: "登录",
  going: "正在登录…",
  signUp: "注册账号",
  signUpGo: "注册并登录",
  scan: "扫码登录",
  withPassword: "使用账号密码登录",
  scanFailed: "无法扫描",
  alone: "暂不加入",
  aloneWhat: "这台电脑可以单独使用，之后随时加入或新建网络",
  worldName: "网络名称",
  serverName: "服务器名称",
  serverNameReading: "正在读取它的主机名…",
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
  placeMissing: "Fastctx 组件未安装",
  place: "安装",
  problems: zhProblems,
};

export const enLogin: LoginWords = {
  lede: "Your computers, and the Codex and Claude on them, together in one network.",
  title: "Log in",
  join: "Join a network",
  joinWhat: "Log in with a hub address and account",
  create: "Make it on this computer",
  createWhat: "This computer is its hub",
  remote: "Make it on a server",
  remoteWhat: "Over SSH, a server becomes its hub",
  hubIs: "The hub knows every device in the network, keeps who may use which machine, and passes on what the machines say to each other.",
  createHub: "This computer will be the hub and must stay on: while it sleeps or is off, no other device can connect.",
  remoteHub: "This server will be the hub and must stay on: while it is down, no other device can connect.",
  server: "Server address",
  sshPort: "SSH port",
  sshUser: "User",
  signInBy: "Sign in with",
  keyFile: "Private key file",
  keyPassword: "Key passphrase",
  keyMissing: "Choose a private key file",
  chooseKey: "Choose a file",
  optional: " (optional)",
  passwordFloor: enPasswordFloor,
  next: "Next",
  firstTime: "First time on this server",
  fingerprintWhat: "The server's fingerprint",
  trust: "Trust and go on",
  fingerprintChanged: "This server's fingerprint differs from the one this computer recorded; not connected",
  sudoPassword: "sudo password",
  sudoWanted: "sudo needs a password",
  portClosed: (port) => `Port ${port} can't be reached; allow it in the firewall or security group`,
  sshRefused: "The server didn't accept this user and password or key",
  sshUnreachable: "Can't reach this server's SSH port",
  sudoWrong: "Wrong sudo password",
  notLinux: "This server isn't Linux",
  notArch: "This server's architecture isn't supported; only x86_64 and aarch64 are",
  addGo: "Add",
  adding: "Adding…",
  remoteSteps: { connect: "Connect", check: "Look at the server", upload: "Upload the program", hub: "Set up the network", login: "Sign this computer in", join: "Add the server", arrive: "Move the hub there" },
  another: "Log in to another account",
  hub: "Hub address",
  account: "Account",
  password: "Password",
  passwordAgain: "Confirm password",
  passwordsDiffer: "Passwords don't match",
  passwordRule: enPasswordRule,
  strength: enStrength,
  name: "Computer name",
  phoneName: "Phone name",
  nameSaid: enNameSaid,
  inField: (field, said) => `${field}: ${said}`,
  go: "Log in",
  going: "Logging in…",
  signUp: "Sign up",
  signUpGo: "Sign up and log in",
  scan: "Scan to log in",
  withPassword: "Use account and password",
  scanFailed: "Couldn't scan",
  alone: "Not now",
  aloneWhat: "Use this computer on its own and join or create a network any time",
  worldName: "Network name",
  serverName: "Server name",
  serverNameReading: "Reading its host name…",
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
  placeMissing: "Fastctx components aren't installed",
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
  /** What the hub is, in one line in its details. */
  hubIs: string;
  hubTitle(name: string): string;
  hubMachines(n: number): string;
  /** The account's level on a hub, and what it does there. */
  levelSuper: string;
  levelMember: string;
  inviteDevice: string;
  manageHub: string;
  /** How this computer reaches a hub. */
  linkOnline(address: string): string;
  linkVia(through: string, address: string): string;
  linkConnecting(error: string | null): string;
  linkStopped(reason: string): string;
  /** Where this computer stands on a hub. */
  registeredUnder(account: string): string;
  onlyLogins(accounts: string): string;
  via(through: string): string;
  /** Under a machine's name on the drawing, when it reaches the hub
   *  through another. */
  viaShort(through: string): string;
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
  remove: string;
  /** An agent lent to the account, on a machine of another's: the tag on
   *  the map, the one line on its panel, and the saying behind both. */
  lentShort(owner: string): string;
  borrowedNote: string;
  borrowedSaid(owner: string): string;
  /** A lent login its machine does not list. */
  borrowedGone: string;
  talk: string;
  /** On its own, in no network: the film of what a network is for, and the
   *  two ways into one. */
  film: NetworkWords;
  /** In a network: the film again, over the drawing, and closing it. */
  filmAgain: string;
  filmAgainWhat: string;
  filmClose: string;
  newNetwork: string;
  joinNetwork: string;
  /** The drawing as a canvas: what it is to a screen reader, its corner
   *  controls, the dashed ring that adds a machine and what it says while
   *  one is on its way, and what floats beside one chosen. */
  map: string;
  zoomOut: string;
  zoomIn: string;
  fit: string;
  scale: string;
  addMachine: string;
  waitingJoin: string;
  details: string;
  closeDetails: string;
  actions: string;
  /** A share or loan taken back from the drawing, and giving it again. */
  tookBack(to: string): string;
  undo: string;
  confirm: string;
  more: string;
  /** On a machine others may connect to directly, for a pointer resting on
   *  its ring. */
  takesDirect: string;
  /** The groups round a hub, written along their arcs: the account's own
   *  machines, and those an agent is lent on from another. Others' are
   *  named by `ownedBy`. */
  mineGroup: string;
  lentGroup(owner: string): string;
  /** Under a group's name on the drawing: how many, how many online. */
  groupSub(count: number, online: number): string;
  /** A machine's direct links, in its details. */
  directRow: string;
  directUp(peer: string, ms: number | undefined): string;
  directDown(peer: string, why: string | undefined): string;
  takesDirectAt(addresses: string): string;
  /** Under the drawing, for a mouse: how to zoom and how to move it. */
  hintZoom: string;
  hintPan: string;
  /** Several machines framed with the mouse, and what is done to them
   *  together. */
  framedTitle(n: number): string;
  framedShare: string;
  framedRemove: string;
  framedClear: string;
  framedShareTitle(n: number): string;
  framedRemoveTitle(n: number): string;
  framedRemoveWhat(names: string[]): string;
  /** Letting other machines connect to one directly, and no longer: the
   *  administrator's, from beside its ring. */
  allowDirect: string;
  stopDirect: string;
  directTitle(machine: string): string;
  directAddresses: string;
  directSeen: string;
  directCard: string;
  directPort: string;
  directPickOne: string;
  directPortWrong: string;
  directWhat: string;
  directGo: string;
  directChecking: string;
  directAllowed(machine: string): string;
  stopDirectTitle(machine: string): string;
  stopDirectSaid: string;
  directStopped(machine: string): string;
  /** Moving the server to another machine: the administrator's, from beside
   *  the server on the drawing (`world-gui.md`, 把中枢服务器搬到别处). */
  moveHub: string;
  moveChoose: string;
  moveNewServer: string;
  /** Why a machine cannot take the server, from what the server says. */
  moveWhyNot: { offline: string; hosts: string; moving(to: string): string; older: string; system: string; notOwn: string; other: string };
  moveSleeps: string;
  moveTitle(to: string): string;
  /** What goes with the server, as a row of tags. */
  moveCarries: string[];
  moveSaid: string;
  moveWhat: string;
  moveTypeName(hub: string): string;
  moveGo: string;
  moveNewTitle: string;
  moveNext: string;
  moveStages: Record<"preparing" | "service" | "reaching" | "copying" | "switching" | "following" | "done" | "cancelled", string>;
  moveTo(to: string): string;
  moveMoved(to: string): string;
  moveInstallHere: string;
  moveInstall: string;
  moveInstallThere(machine: string): string;
  /** The command that installs the server there, on its system. */
  moveInstallCommand(os: string): string;
  moveCopy: string;
  moveCopied: string;
  moveCall: string;
  moveRetry: string;
  moveFollowing(machines: number, devices: number): string;
  moveAllFollowed: string;
  moveOldForwards: string;
  moveOldStopped: string;
  moveOldDeleted: string;
  moveStop: string;
  moveStopTitle: string;
  moveStopSaid(machines: string[]): string;
  moveDelete: string;
  moveDeleteTitle: string;
  moveDeleteSaid: string;
  /** The hub's record of changes, in the drawing's corner. */
  log: string;
  logEmpty: string;
  logEarlier: string;
  logFailed: string;
  /** Who asked: an account, and the device it asked from. */
  logBy(account: string | null, device: string | null): string;
  /** A request the hub could not tie to the account it named. */
  logUnverified: string;
  /** What a record changed, in a line; `null` for what is not about
   *  machines (accounts, passwords, projects). */
  logSaid(action: MachineLogAction): string | null;
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
  hubIs: "认出网络里的每台设备，管着谁能用哪台机器，机器之间的话经它转达",
  hubTitle: (name) => `中枢服务器 ${name}`,
  hubMachines: (n) => `${n} 台机器`,
  levelSuper: "管理员",
  levelMember: "成员",
  inviteDevice: "添加设备",
  manageHub: "管理账号",
  linkOnline: (address) => `已连上 · ${address}`,
  linkVia: (through, address) => `经 ${through} 连上 · ${address}`,
  linkConnecting: () => "连接中",
  linkStopped: () => "已被中枢服务器移除",
  registeredUnder: (account) => `${account} 的机器`,
  onlyLogins: (accounts) => `已登录 ${accounts}`,
  via: (through) => `经 ${through} 连着中枢服务器`,
  viaShort: (through) => `经 ${through}`,
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
  remove: "移除",
  lentShort: (owner) => `来自 ${owner}`,
  borrowedNote: "在你的机器上工作",
  borrowedSaid: (owner) => `${owner} 借给你；它在你的机器上工作，不接触 ${owner} 的这台电脑`,
  borrowedGone: "该登录已不可用",
  talk: "新对话",
  film: {
    captions: ["你的智能体在这台电脑上干活", "几台机器连到一台中枢服务器，就成了一个网络", "它在每一台上都能干活：写一句 on gpu，活就在那台跑", "合上电脑，那边照样跑完，结果等它回来", "文件在机器之间直接搬，机器也能共享给别人"],
    thought: "Thought for 2s",
    ran: "运行",
    hub: "中枢服务器",
    shared: "共享给 bo",
  },
  filmAgain: "再看一遍",
  filmAgainWhat: "网络能做什么",
  filmClose: "关闭",
  newNetwork: "新建网络",
  joinNetwork: "加入网络",
  map: "机器图：按住滚轮拖动，Ctrl 加滚轮缩放，Tab 逐台选中",
  zoomOut: "缩小",
  zoomIn: "放大",
  fit: "看全",
  scale: "比例",
  addMachine: "添加机器",
  waitingJoin: "等待加入",
  details: "详情",
  closeDetails: "收起详情",
  actions: "操作",
  tookBack: (to) => `已收回 ${to}`,
  undo: "撤回",
  confirm: "确定",
  more: "细节",
  takesDirect: "别的机器可以直接连到这台",
  mineGroup: "我的",
  lentGroup: (owner) => `来自 ${owner}`,
  groupSub: (count, online) => `${count} 台 · ${online} 在线`,
  directRow: "直连",
  directUp: (peer, ms) => `${peer}${ms !== undefined ? ` ${ms} ms` : ""}`,
  directDown: (peer, why) => `${peer} 断了${why ? `：${why}` : ""}`,
  takesDirectAt: (addresses) => `别的机器经 ${addresses} 直接连它`,
  hintZoom: "缩放",
  hintPan: "按住拖动",
  framedTitle: (n) => `选了 ${n} 台`,
  framedShare: "一起共享",
  framedRemove: "一起移除",
  framedClear: "取消选择",
  framedShareTitle: (n) => `共享这 ${n} 台`,
  framedRemoveTitle: (n) => `移除这 ${n} 台？`,
  framedRemoveWhat: (names) => `${names.join("、")} 将断开中枢服务器并退出所有账号，共享与借出随之失效。机器上的数据不受影响。`,
  allowDirect: "允许直接连到这台",
  stopDirect: "不再允许直接连到这台",
  directTitle: (machine) => `允许直接连到 ${machine}`,
  directAddresses: "别的机器连它的地址",
  directSeen: "中枢服务器看到的",
  directCard: "网卡",
  directPort: "端口",
  directPickOne: "至少选一个地址",
  directPortWrong: "端口是 1 到 65535 之间的数",
  directWhat: "开了之后，别的机器和它之间的往来直接走，不经中枢服务器；哪台到中枢服务器的线断了，也可以经它绕过去。按下之后中枢服务器从外面试连一次，连不上会说卡在哪一层。防火墙、安全组或路由器要放行这个端口。",
  directGo: "允许",
  directChecking: "中枢服务器正在试着连它",
  directAllowed: (machine) => `已允许直接连到 ${machine}`,
  stopDirectTitle: (machine) => `不再允许直接连到 ${machine}？`,
  stopDirectSaid: "别的机器和它之间改经中枢服务器转。",
  directStopped: (machine) => `已不再允许直接连到 ${machine}`,
  moveHub: "搬到…",
  moveChoose: "选一台接手中枢服务器",
  moveNewServer: "一台新服务器",
  moveWhyNot: {
    offline: "离线",
    hosts: "中枢服务器就在这台",
    moving: (to) => `正在搬到 ${to}`,
    older: "这台的 Fastctx 版本太旧",
    system: "中枢服务器只能装在 Windows 和 Linux 上",
    notOwn: "只有管理员自己的机器能接手",
    other: "不能接手",
  },
  moveSleeps: "这台会睡眠，睡着时整个网络都连不上中枢服务器",
  moveTitle: (to) => `把中枢服务器搬到 ${to}？`,
  moveCarries: ["账号", "机器", "权限", "项目", "信"],
  moveSaid: "谁都不用重新登录。",
  moveWhat: "中枢服务器的整个目录搬过去，身份不变。拷贝时照常服务，只在切换那一下停一停；之后旧的位置把连过来的机器转过去，直到每台都跟过来。旧的数据停用、不删，在中枢服务器详情里删。",
  moveTypeName: (hub) => `输入网络名称 ${hub} 确认`,
  moveGo: "搬过去",
  moveNewTitle: "把中枢服务器搬到一台新服务器",
  moveNext: "下一步",
  moveStages: { preparing: "准备", service: "装上中枢服务器", reaching: "连上新位置", copying: "拷贝记录", switching: "切换", following: "等各台跟过来", done: "完成", cancelled: "已取消" },
  moveTo: (to) => `正在搬到 ${to}`,
  moveMoved: (to) => `已搬到 ${to}`,
  moveInstallHere: "这台电脑要装上中枢服务器，需要管理员确认一次。",
  moveInstall: "装上",
  moveInstallThere: (machine) => `在 ${machine} 上的 Fastctx 里点「装上」，或在它的终端里运行：`,
  moveInstallCommand: (os) => (os === "windows" ? "fastctx hub arrive" : "sudo fastctx hub arrive"),
  moveCopy: "复制",
  moveCopied: "已复制",
  moveCall: "取消搬家",
  moveRetry: "重试",
  moveFollowing: (machines, devices) => [machines ? `${machines} 台机器` : "", devices ? `${devices} 台设备` : ""].filter(Boolean).join("、") + "还没跟过来",
  moveAllFollowed: "都已跟过来",
  moveOldForwards: "旧的位置在转发",
  moveOldStopped: "旧的位置已停下，数据停用未删",
  moveOldDeleted: "旧数据已删",
  moveStop: "停下旧的转发",
  moveStopTitle: "停下旧位置的转发？",
  moveStopSaid: (machines) => `${machines.length ? `${machines.join("、")} 还没跟过来，` : ""}停下之后要在那里重新登录，登录后还是同一台机器。`,
  moveDelete: "删掉旧数据",
  moveDeleteTitle: "删掉旧位置的数据？",
  moveDeleteSaid: "旧位置的中枢服务器数据删掉后，就不能退回去了。",
  log: "变更记录",
  logEmpty: "暂无记录",
  logEarlier: "更早",
  logFailed: "读取失败",
  logBy: (account, device) => [account, device].filter(Boolean).join(" · "),
  logUnverified: "未验证",
  logSaid: (a) => {
    switch (a.change) {
      case "share":
        return a.shared ? `把 ${a.machine} 共享给 ${a.to}` : `收回 ${a.machine} 对 ${a.to} 的共享`;
      case "lend":
        if (!a.lent) return `收回借给 ${a.to} 的 ${agentName(a.agent)}`;
        return `把 ${a.machine} 上的 ${agentName(a.agent)} 借给 ${a.to}${a.until ? `，到 ${zhMoment.stamp(new Date(a.until))}` : ""}`;
      case "lend_ended":
        return `借给 ${a.to} 的 ${agentName(a.agent)} 到期收回`;
      case "plan":
        return a.plan ? `把 ${a.machine} 上 ${agentName(a.agent)} 的套餐标为 ${a.plan}` : `去掉 ${a.machine} 上 ${agentName(a.agent)} 的套餐标记`;
      case "stop_acting":
        return `撤销 ${a.machine} 访问的权限`;
      case "relays":
        return `改了 ${a.machine} 连中枢服务器的方式`;
      case "relay_at":
        return `改了 ${a.machine} 对外的地址`;
      case "direct":
        return a.at.length ? `允许直接连到 ${a.machine}` : `不再允许直接连到 ${a.machine}`;
      case "remove":
        return `移除 ${a.name}`;
      case "token":
        return a.register ? `生成加入命令${a.name ? ` · ${a.name}` : ""}` : "生成登录二维码";
      case "joined":
        return a.registered ? `${a.name} 加入` : `${a.name} 登录`;
      case "left":
        return `${a.name} 离开`;
      case "allow_acting":
        return `${a.machine} 上的 ${agentName(a.agent)} 获准访问`;
      case "acting_stopped":
        return `${a.machine} 不再能替 ${a.account} 访问`;
      default:
        return null;
    }
  },
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
  hubIs: "It knows every device in the network, keeps who may use which machine, and passes on what the machines say to each other",
  hubTitle: (name) => `Hub ${name}`,
  hubMachines: (n) => `${n} machine${n === 1 ? "" : "s"}`,
  levelSuper: "administrator",
  levelMember: "member",
  inviteDevice: "Add device",
  manageHub: "Accounts",
  linkOnline: (address) => `connected · ${address}`,
  linkVia: (through, address) => `connected through ${through} · ${address}`,
  linkConnecting: () => "connecting",
  linkStopped: () => "removed by the hub",
  registeredUnder: (account) => `${account}'s machine`,
  onlyLogins: (accounts) => `signed in to ${accounts}`,
  via: (through) => `reaches the hub through ${through}`,
  viaShort: (through) => `via ${through}`,
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
  remove: "Remove",
  lentShort: (owner) => `from ${owner}`,
  borrowedNote: "Works on your machines",
  borrowedSaid: (owner) => `Lent to you by ${owner}; it works on your machines and never touches ${owner}'s computer`,
  borrowedGone: "Login unavailable",
  talk: "New conversation",
  film: {
    captions: ["Your agent works on this computer", "Machines that reach one hub make a network", "It works on any of them: write on gpu, and the job runs there", "Close the computer: the job runs on, and its result waits", "Files go straight between machines, and a machine can be shared"],
    thought: "Thought for 2s",
    ran: "Ran",
    hub: "Hub",
    shared: "Shared with bo",
  },
  filmAgain: "Watch again",
  filmAgainWhat: "What a network does",
  filmClose: "Close",
  newNetwork: "New network",
  joinNetwork: "Join a network",
  map: "Machines: hold the wheel to move, Ctrl and the wheel to zoom, Tab to each machine",
  zoomOut: "Zoom out",
  zoomIn: "Zoom in",
  fit: "Fit",
  scale: "Scale",
  addMachine: "Add a machine",
  waitingJoin: "Waiting to join",
  details: "Details",
  closeDetails: "Hide details",
  actions: "Actions",
  tookBack: (to) => `Revoked ${to}`,
  undo: "Undo",
  confirm: "Confirm",
  more: "Details",
  takesDirect: "Other machines may connect to it directly",
  mineGroup: "Mine",
  lentGroup: (owner) => `From ${owner}`,
  groupSub: (count, online) => `${count} · ${online} online`,
  directRow: "Direct",
  directUp: (peer, ms) => `${peer}${ms !== undefined ? ` ${ms} ms` : ""}`,
  directDown: (peer, why) => `${peer} down${why ? `: ${why}` : ""}`,
  takesDirectAt: (addresses) => `Others connect to it directly at ${addresses}`,
  hintZoom: "Zoom",
  hintPan: "Hold to move",
  framedTitle: (n) => `${n} chosen`,
  framedShare: "Share together",
  framedRemove: "Remove together",
  framedClear: "Clear",
  framedShareTitle: (n) => `Share these ${n}`,
  framedRemoveTitle: (n) => `Remove these ${n}?`,
  framedRemoveWhat: (names) => `${names.join(", ")} will leave the hub and every account; their shares and loans end. Nothing on the machines is touched.`,
  allowDirect: "Allow direct connections",
  stopDirect: "Stop direct connections",
  directTitle: (machine) => `Allow direct connections to ${machine}`,
  directAddresses: "Where others connect to it",
  directSeen: "as the hub sees it",
  directCard: "network card",
  directPort: "Port",
  directPickOne: "Choose at least one address",
  directPortWrong: "A port is a number from 1 to 65535",
  directWhat: "Once allowed, what goes between it and other machines goes straight, not through the hub, and a machine whose own line to the hub is down can reach the hub through it. The hub then tries to connect to it once from outside and says where it was stopped if it can't. Allow the port in the firewall, security group or router.",
  directGo: "Allow",
  directChecking: "The hub is trying to reach it",
  directAllowed: (machine) => `Allowed direct connections to ${machine}`,
  stopDirectTitle: (machine) => `Stop direct connections to ${machine}?`,
  stopDirectSaid: "What goes between it and other machines will go through the hub.",
  directStopped: (machine) => `No more direct connections to ${machine}`,
  moveHub: "Move to…",
  moveChoose: "Choose the machine to take the hub",
  moveNewServer: "A new server",
  moveWhyNot: {
    offline: "Offline",
    hosts: "The hub is on it",
    moving: (to) => `Moving to ${to}`,
    older: "Its Fastctx is too old",
    system: "The hub runs on Windows and Linux",
    notOwn: "Only the administrator's own machines can take it",
    other: "Cannot take it",
  },
  moveSleeps: "It sleeps; while it does, no machine reaches the hub",
  moveTitle: (to) => `Move the hub to ${to}?`,
  moveCarries: ["Accounts", "Machines", "Rights", "Projects", "Letters"],
  moveSaid: "Nobody signs in again.",
  moveWhat: "The hub's whole directory moves, its identity unchanged. It keeps serving while it copies and pauses only for the switch; then the old place forwards each machine that comes to it until all have followed. The old data is disabled, not deleted; delete it in the hub's details.",
  moveTypeName: (hub) => `Type ${hub} to confirm`,
  moveGo: "Move",
  moveNewTitle: "Move the hub to a new server",
  moveNext: "Next",
  moveStages: { preparing: "Prepare", service: "Install the hub", reaching: "Reach the new place", copying: "Copy the records", switching: "Switch", following: "Wait for every machine", done: "Done", cancelled: "Called off" },
  moveTo: (to) => `Moving to ${to}`,
  moveMoved: (to) => `Moved to ${to}`,
  moveInstallHere: "This computer installs the hub, after one administrator confirmation.",
  moveInstall: "Install",
  moveInstallThere: (machine) => `In Fastctx on ${machine}, press Install, or run in its terminal:`,
  moveInstallCommand: (os) => (os === "windows" ? "fastctx hub arrive" : "sudo fastctx hub arrive"),
  moveCopy: "Copy",
  moveCopied: "Copied",
  moveCall: "Call off the move",
  moveRetry: "Try again",
  moveFollowing: (machines, devices) => [machines ? `${machines} machine${machines === 1 ? "" : "s"}` : "", devices ? `${devices} device${devices === 1 ? "" : "s"}` : ""].filter(Boolean).join(" and ") + " not followed yet",
  moveAllFollowed: "Everything followed",
  moveOldForwards: "The old place forwards",
  moveOldStopped: "The old place stopped; its data is disabled, not deleted",
  moveOldDeleted: "The old data is deleted",
  moveStop: "Stop forwarding",
  moveStopTitle: "Stop the old place forwarding?",
  moveStopSaid: (machines) => `${machines.length ? `${machines.join(", ")} ${machines.length === 1 ? "has" : "have"} not followed; ` : ""}after this they sign in again there, and stay the same machines.`,
  moveDelete: "Delete the old data",
  moveDeleteTitle: "Delete the old place's data?",
  moveDeleteSaid: "Once the hub's data at the old place is deleted, there is no going back to it.",
  log: "Changes",
  logEmpty: "No changes yet",
  logEarlier: "Earlier",
  logFailed: "Couldn't load",
  logBy: (account, device) => [account, device].filter(Boolean).join(" · "),
  logUnverified: "unverified",
  logSaid: (a) => {
    switch (a.change) {
      case "share":
        return a.shared ? `Shared ${a.machine} with ${a.to}` : `Stopped sharing ${a.machine} with ${a.to}`;
      case "lend":
        if (!a.lent) return `Took back ${agentName(a.agent)} from ${a.to}`;
        return `Lent ${agentName(a.agent)} on ${a.machine} to ${a.to}${a.until ? ` until ${enMoment.stamp(new Date(a.until))}` : ""}`;
      case "lend_ended":
        return `${agentName(a.agent)} lent to ${a.to} came back at its end`;
      case "plan":
        return a.plan ? `Marked ${agentName(a.agent)} on ${a.machine} as ${a.plan}` : `Cleared the plan of ${agentName(a.agent)} on ${a.machine}`;
      case "stop_acting":
        return `Revoked ${a.machine}'s access`;
      case "relays":
        return `Changed how ${a.machine} reaches the hub`;
      case "relay_at":
        return `Changed ${a.machine}'s public address`;
      case "direct":
        return a.at.length ? `Allowed direct connections to ${a.machine}` : `No more direct connections to ${a.machine}`;
      case "remove":
        return `Removed ${a.name}`;
      case "token":
        return a.register ? `Made a join command${a.name ? ` · ${a.name}` : ""}` : "Made a sign-in QR code";
      case "joined":
        return a.registered ? `${a.name} joined` : `${a.name} signed in`;
      case "left":
        return `${a.name} left`;
      case "allow_acting":
        return `${agentName(a.agent)} on ${a.machine} was allowed access`;
      case "acting_stopped":
        return `${a.machine} no longer acts for ${a.account}`;
      default:
        return null;
    }
  },
};

/** The sheets on a machine: whom its owner shares it with, whom it lends
 *  one login of an agent on it, and removing it. */
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
  /** The lend sheet's own: whom to lend to, until when, and changing it. */
  lendWho: string;
  lendUntil: string;
  changeUntil: string;
  untilPassed: string;
  save: string;
  cancel: string;
  when: WhenWords;
  moment: MomentWords;
  nobodyYet: string;
  to: string;
  toHint: string;
  takeBack: string;
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
  lendWho: "借给",
  lendUntil: "截止",
  changeUntil: "改截止时间",
  untilPassed: "这个截止时间已经过了",
  save: "保存",
  cancel: "取消",
  when: zhWhen,
  moment: zhMoment,
  nobodyYet: "暂无",
  to: "账号",
  toHint: "对方的账号名",
  takeBack: "收回",
  removeTitle: (machine) => `移除 ${machine}？`,
  removeWhat: (machine) => `${machine} 将断开中枢服务器并退出所有账号，共享与借出随之失效。机器上的数据不受影响。`,
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
  lendWho: "Lend to",
  lendUntil: "Ends",
  changeUntil: "Change when it ends",
  untilPassed: "That time has already passed",
  save: "Save",
  cancel: "Cancel",
  when: enWhen,
  moment: enMoment,
  nobodyYet: "None yet",
  to: "Account",
  toHint: "Their account name",
  takeBack: "Revoke",
  removeTitle: (machine) => `Remove ${machine}?`,
  removeWhat: (machine) => `${machine} will disconnect from the hub and sign out of every account; its shares and loans end. Data on it is untouched.`,
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
  /** A computer as its node says it is. */
  forms: Record<"desktop" | "laptop" | "server", string>;
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
  passwordFloor: string;
  strength: [string, string, string];
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
  hub: "中枢服务器",
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
  forms: { desktop: "台式机", laptop: "笔记本", server: "服务器" },
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
      ? `${name} 将断开中枢服务器并退出所有账号，共享与借出随之失效。机器上的数据不受影响。`
      : `${name} 将退出 ${account}。`,
  removeDeviceGo: "移除",
  invite: "添加设备",
  inviteWhat: "手机扫码，电脑用中枢服务器地址和账号登录，Linux 主机运行一行命令",
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
  passwordFloor: zhPasswordFloor,
  strength: zhStrength,
  passwordGo: "保存",
  passwordDone: "密码已修改",
  logout: "退出账号",
  logoutTitle: (login) => `退出 ${login}？`,
  logoutWhat: (login, registered) =>
    registered
      ? `这台电脑将退出 ${login}，不再作为它的机器，共享与借出随之失效。电脑上的数据不受影响。`
      : `这台设备将退出 ${login}。`,
  logoutGo: "退出",
  adminPage: "管理中枢服务器",
  another: "添加账号",
  none: "未加入网络",
  logIn: "加入",
  working: "处理中…",
  close: "关闭",
  failed: failure(zhProblems, "操作失败"),
};

export const enAccount: AccountWords = {
  title: "Account",
  hub: "Hub",
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
  forms: { desktop: "desktop", laptop: "laptop", server: "server" },
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
      ? `${name} will disconnect from the hub and sign out of every account; its shares and loans end. Data on it is untouched.`
      : `${name} will be signed out of ${account}.`,
  removeDeviceGo: "Remove",
  invite: "Add device",
  inviteWhat: "Phones scan a code, computers sign in with the hub address and account, Linux hosts run one command",
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
  passwordFloor: enPasswordFloor,
  strength: enStrength,
  passwordGo: "Save",
  passwordDone: "Password changed",
  logout: "Sign out",
  logoutTitle: (login) => `Sign out of ${login}?`,
  logoutWhat: (login, registered) =>
    registered
      ? `This computer will sign out of ${login} and stop being its machine; its shares and loans end. Data on it is untouched.`
      : `This device will sign out of ${login}.`,
  logoutGo: "Sign out",
  adminPage: "Manage hub",
  another: "Add account",
  none: "Not in a network",
  logIn: "Join",
  working: "Working…",
  close: "Close",
  failed: failure(enProblems, "Couldn't complete"),
};

/** The sheet that adds another device to an account. */
export interface TokenWords {
  title: string;
  kinds: Record<"computer" | "phone" | "server", string>;
  /** A Linux host joins over SSH from here, or by the line it runs. */
  ways: Record<"ssh" | "line", string>;
  /** A server added over SSH. */
  added(machine: string): string;
  hub: string;
  account: string;
  name: string;
  nameAuto: string;
  nameSaid: NameSaid;
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
  ways: { ssh: "经 SSH", line: "一行命令" },
  added: (machine) => `${machine} 已加入`,
  hub: "中枢服务器地址",
  account: "账号",
  name: "主机名称",
  nameAuto: "自动",
  nameSaid: zhNameSaid,
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
  ways: { ssh: "Over SSH", line: "One command" },
  added: (machine) => `${machine} joined`,
  hub: "Hub address",
  account: "Account",
  name: "Host name",
  nameAuto: "automatic",
  nameSaid: enNameSaid,
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
  /** Said before sign-up is opened, which lets anyone who reaches the
   *  server in. */
  signUpTitle: string;
  signUpWhat: string;
  signUpGo: string;
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
  passwordFloor: string;
  strength: [string, string, string];
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
  title: "管理中枢服务器",
  hub: (name) => `中枢服务器 ${name}`,
  superPart: "管理员",
  superWhat: "可以使用这个网络里的所有机器；管理员只有一位，可以转交",
  transfer: "转交",
  transferTitle: "转交管理员",
  transferWhat: "转交后你将成为成员，对方可以使用这个网络里的所有机器。",
  transferTo: "转交给",
  transferGo: "转交",
  noOther: "没有其他账号",
  selfRegistration: "开放注册",
  selfRegistrationWhat: "能连上中枢服务器的人都可以自行注册，注册的账号为成员。建议关闭，由管理员新建账号",
  signUpTitle: "开放注册？",
  signUpWhat:
    "开放后，能连上这台中枢服务器的任何人都能自己注册账号。新账号是成员，可以把自己的电脑加进来、开项目。中枢服务器在公网上时，陌生人也能注册。\n\n建议保持关闭，需要时用「新建账号」为对方开一个。",
  signUpGo: "仍要开放",
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
  passwordFloor: zhPasswordFloor,
  strength: zhStrength,
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
  title: "Manage hub",
  hub: (name) => `Hub ${name}`,
  superPart: "Administrator",
  superWhat: "Can use every machine in the network; there is one administrator, and the role can be transferred",
  transfer: "Transfer",
  transferTitle: "Transfer administrator",
  transferWhat: "You'll become a member, and they'll be able to use every machine in the network.",
  transferTo: "Transfer to",
  transferGo: "Transfer",
  noOther: "No other accounts",
  selfRegistration: "Open sign-up",
  selfRegistrationWhat: "Anyone who can reach the hub can sign up; new accounts are members. Best kept off, with accounts made by the administrator",
  signUpTitle: "Open sign-up?",
  signUpWhat:
    "Anyone who can reach this hub will be able to create an account. New accounts are members: they can add their own computers and open projects. If the hub is on the internet, strangers can sign up too.\n\nWe recommend keeping it off and making accounts for people with New account.",
  signUpGo: "Open anyway",
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
  passwordFloor: enPasswordFloor,
  strength: enStrength,
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
