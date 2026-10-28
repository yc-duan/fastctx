// What the pages about accounts say (`world-gui.md`, 账号、机器、共享与协助):
// logging in; the machines a login may use, the agents assisting it, and what
// is set on them; the account itself, with its level, its devices, who came
// into it, the machines whose agents may reach its machines, the one-time
// tokens that let another device in, its password and its projects; and the
// super administrator's page of the hub. In the user's words
// (`world-gui.md`, 用词).

import { wholeDollars } from "../world/money";
import type { MachineFindWords } from "./machine-find";
import type { GroupWords } from "./machine-groups";
import type { PasswordProblem } from "./password";
import { enProblems, failure, zhProblems, type ProblemWords } from "./problem";
import type { MachineLogAction } from "../world/machine-log";
import type { Unfit } from "../world/accounts";
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
  proxy: string;
  proxyWhat: string;
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
  remoteSteps: Record<"connect" | "check" | "upload" | "hub" | "login" | "join" | "arrive" | "withdraw", string>;
  /** The four steps of making a World on a server, over its form. */
  remoteStages: [string, string, string, string];
  /** On the button while signing in to the server to look at it. */
  looking: string;
  /** Where other devices will reach the network made. */
  networkAt(address: string): string;
  portTaken: string;
  portRange: string;
  /** Under the administrator's fields: who it is, or, for a World the
   *  server keeps already, that it is the one it was made with. */
  adminWhat: string;
  adminAgain: string;
  /** The name this computer takes in the World made. */
  hereName: string;
  sameNames: string;
  /** A server that keeps a World already: by its name, or one left half
   *  made; finishing it, signing in to it, taking it down. */
  found(world: string): string;
  foundHalf: string;
  resumeGo: string;
  signInThere: string;
  withdrawGo: string;
  withdrawTitle(world: string | null): string;
  withdrawWhat: string;
  withdrawType(world: string): string;
  withdrawConfirm: string;
  withdrawing: string;
  /** Where only a line run on the server takes it down. */
  withdrawByHand: string;
  /** After a step that failed: again, or back to what was asked. */
  retry: string;
  backToEdit: string;
  /** In what is being made: the network, its administrator, the server and
   *  this computer. */
  thisComputer: string;
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
  /** A computer joins with an account and its password, or by a code
   *  (`world-network.md`, 受管机器): the two tabs, the code's field, and the
   *  card of what it lets this computer into before it is used. */
  byPassword: string;
  byCode: string;
  code: string;
  peeking: string;
  codeOwner: string;
  codeNetwork: string;
  managedTitle: string;
  managedCan(account: string): string;
  managedNot: string;
  codeAs(account: string): string;
  codeUnknown: string;
  codeGo: string;
  /** Why this Fastctx cannot become a managed machine, and the way out. */
  fresh: string;
  unfit: Record<Unfit["why"], string>;
  removeFirst: string;
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

export const zhPasswordRule: Record<PasswordProblem, string> = { short: "至少 8 位", "one-kind": "需包含字母、数字、符号中的两种", name: "不能包含账号名", common: "这个密码太常见" };

export const enPasswordRule: Record<PasswordProblem, string> = { short: "At least 8 characters", "one-kind": "Use two of letters, digits and symbols", name: "Can't contain the account name", common: "This password is too common" };
// The bar a new password must clear, and how strong one is, wherever one is set.
export const zhPasswordFloor = "至少 8 位，字母、数字、符号中至少两种";
export const enPasswordFloor = "At least 8 characters, with two of letters, numbers and symbols";
export const zhStrength: [string, string, string] = ["强度一般", "强度较高", "强度高"];
export const enStrength: [string, string, string] = ["Fair", "Good", "Strong"];

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
  proxy: "代理",
  proxyWhat: "服务器要经 HTTP 代理才能上网时填，比如 http://127.0.0.1:7890；它以后一直经这个代理连中枢服务器",
  portClosed: (port) => `连不上端口 ${port}，防火墙或安全组要放行它`,
  sshRefused: "服务器没有接受这个用户名和密码或钥匙",
  sshUnreachable: "连不上这台服务器的 SSH 端口",
  sudoWrong: "sudo 密码不对",
  notLinux: "这台服务器不是 Linux",
  notArch: "这台服务器的架构不受支持，只支持 x86_64 与 aarch64",
  addGo: "添加",
  adding: "正在添加…",
  remoteSteps: { connect: "连接服务器", check: "查看服务器", upload: "上传程序", hub: "建网络", login: "这台电脑登录", join: "服务器加入", arrive: "中枢服务器搬过去", withdraw: "撤掉网络" },
  remoteStages: ["服务器", "网络", "管理员", "机器"],
  looking: "正在连接…",
  networkAt: (address) => `别的设备用 ${address} 连进来`,
  portTaken: "服务器上这个端口已被占用",
  portRange: "端口要在 1 到 65535 之间",
  adminWhat: "网络里唯一的超级管理员，这台电脑用它登录",
  adminAgain: "用建这个网络时的管理员账号登录",
  hereName: "这台电脑的名称",
  sameNames: "两台机器不能同名",
  found: (world) => `这台服务器上已经有网络「${world}」`,
  foundHalf: "这台服务器上有一个没建完的网络",
  resumeGo: "接着建完",
  signInThere: "用管理员登录",
  withdrawGo: "撤掉重来",
  withdrawTitle: (world) => (world ? `撤掉网络「${world}」` : "撤掉没建完的网络"),
  withdrawWhat: "服务器上的 Fastctx 会整个移除：这个网络、它的账号和记录都没了，已经连进来的设备再也连不上。",
  withdrawType: (world) => `输入「${world}」确认`,
  withdrawConfirm: "撤掉",
  withdrawing: "正在撤掉…",
  withdrawByHand: "这台服务器的 sudo 要密码，要在它的终端里运行：",
  retry: "再试",
  backToEdit: "返回修改",
  thisComputer: "这台电脑",
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
  byPassword: "账号密码",
  byCode: "入网码",
  code: "入网码",
  peeking: "正在查看…",
  codeOwner: "主人",
  codeNetwork: "网络",
  managedTitle: "这台电脑将成为受管机器",
  managedCan: (account) => `${account} 那边的智能体能在这个系统账户里跑命令、读写文件`,
  managedNot: "这台电脑自己什么都用不了：没有工作台，也没有网络和账号的管理",
  codeAs: (account) => `以 ${account} 登录`,
  codeUnknown: "用这个入网码加入",
  codeGo: "加入",
  fresh: "受管机器要从全新的 Fastctx 开始。",
  unfit: {
    network: "这台电脑已经在别的网络里。",
    conversations: "这里的 Fastctx 有过对话。",
    integrated: "这台电脑上的智能体接入了 Fastctx。",
    unknown: "查不清这台电脑上的智能体有没有接入 Fastctx。",
  },
  removeFirst: "先彻底移除",
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
  proxy: "Proxy",
  proxyWhat: "Where the server reaches the internet through an HTTP proxy, as http://127.0.0.1:7890; it reaches the hub through it from then on",
  portClosed: (port) => `Port ${port} can't be reached; allow it in the firewall or security group`,
  sshRefused: "The server didn't accept this user and password or key",
  sshUnreachable: "Can't reach this server's SSH port",
  sudoWrong: "Wrong sudo password",
  notLinux: "This server isn't Linux",
  notArch: "This server's architecture isn't supported; only x86_64 and aarch64 are",
  addGo: "Add",
  adding: "Adding…",
  remoteSteps: { connect: "Connect", check: "Look at the server", upload: "Upload the program", hub: "Set up the network", login: "Sign this computer in", join: "Add the server", arrive: "Move the hub there", withdraw: "Take the network down" },
  remoteStages: ["Server", "Network", "Administrator", "Machines"],
  looking: "Connecting…",
  networkAt: (address) => `Other devices connect at ${address}`,
  portTaken: "This port is already in use on the server",
  portRange: "The port is a number from 1 to 65535",
  adminWhat: "The network's one super administrator; this computer signs in with it",
  adminAgain: "Sign in with the administrator account the network was made with",
  hereName: "This computer's name",
  sameNames: "The two machines need different names",
  found: (world) => `This server already holds the network “${world}”`,
  foundHalf: "This server holds a network that was never finished",
  resumeGo: "Finish it",
  signInThere: "Sign in as its administrator",
  withdrawGo: "Take it down and start over",
  withdrawTitle: (world) => (world ? `Take down “${world}”` : "Take down the unfinished network"),
  withdrawWhat: "Fastctx goes from the server entirely: the network, its accounts and records go, and devices connected to it can't connect again.",
  withdrawType: (world) => `Type “${world}” to confirm`,
  withdrawConfirm: "Take down",
  withdrawing: "Taking it down…",
  withdrawByHand: "This server's sudo wants a password; run this in its terminal:",
  retry: "Try again",
  backToEdit: "Back to edit",
  thisComputer: "This computer",
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
  byPassword: "Password",
  byCode: "Code",
  code: "Code",
  peeking: "Checking…",
  codeOwner: "Owner",
  codeNetwork: "Network",
  managedTitle: "This computer becomes a managed machine",
  managedCan: (account) => `Agents on ${account}'s side run commands and read and write files in this system account`,
  managedNot: "This computer uses nothing itself: no workbench, no managing networks or accounts",
  codeAs: (account) => `Log in as ${account}`,
  codeUnknown: "Join with this code",
  codeGo: "Join",
  fresh: "A managed machine starts from a Fastctx never used. ",
  unfit: {
    network: "This computer is on another network.",
    conversations: "This Fastctx has had conversations.",
    integrated: "Agents on this computer have Fastctx in them.",
    unknown: "Whether agents on this computer have Fastctx in them is not known.",
  },
  removeFirst: "Remove everything first",
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
  /** The names a machine had, each still naming it. */
  formerly(names: string[]): string;
  /** A hub on the map, by how this computer reaches it; pointed at, its
   *  title and how many machines are registered on it. */
  hubOnline: string;
  hubConnecting: string;
  /** Since when the line to a hub is down, and why as the kernel said it. */
  hubDownSince(at: string, why: string | null): string;
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
  linkDissolved: string;
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
  /** Whom this account shared its machine with, and whom a login of it
   *  assists, each ended with a press. */
  sharedTo: string;
  assisting: string;
  takeBack(to: string): string;
  /** Gives back the share of another's machine this account holds. */
  leave: string;
  leaveTitle(machine: string): string;
  leaveWhat(owner: string): string;
  endAssist(to: string): string;
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
  assist: string;
  remove: string;
  /** An agent of another's assisting the account, on a machine of theirs:
   *  the tag on the map, the one line on its panel, and the saying behind
   *  both. */
  assistShort(owner: string): string;
  /** A machine of another's whose agents assist the account, as the map and
   *  the list name it: whose agents, never the machine. */
  assistAgents(owner: string): string;
  assistantNote: string;
  assistantSaid(owner: string): string;
  /** An assisting login its machine does not list. */
  assistantGone: string;
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
  /** The bar beside the one chosen: its "⋯", letting it go on a phone,
   *  and the names of the facts on the card of a hub or a phone. */
  moreActs: string;
  closeSpot: string;
  spot: { copy: string; address: string; link: string; machines: string; admin: string; signedIn: string; state: string };
  /** A share or an assistance ended from the drawing, and giving it
   *  again. */
  tookBack(to: string): string;
  undo: string;
  confirm: string;
  more: string;
  /** On a machine others may connect to directly, for a pointer resting on
   *  its ring. */
  takesDirect: string;
  /** The groups round a hub, written along their arcs: the account's own
   *  machines, and those whose agent assists from another. Others' are
   *  named by `ownedBy`. */
  mineGroup: string;
  assistGroup(owner: string): string;
  /** What is done with the groups of machines (`world-network.md`, 机器的分组). */
  groups: GroupWords;
  /** Under a group's name on the drawing: how many, how many online. */
  groupSub(count: number, online: number): string;
  /** On a box seen from afar, after its name: how many; its bar says how
   *  many are online. */
  groupCount(count: number): string;
  /** The list's sections past the account's own: the machines shared with
   *  it, the members' its administrator sees, and those whose agents
   *  assist it. */
  sharedSection: string;
  membersSection: string;
  assistSection: string;
  /** The page's two ways of showing the machines, and what the choice is. */
  viewMap: string;
  viewList: string;
  views: string;
  /** Finding machines by name, by whose they are or by group, and the list
   *  when none matches. */
  find: MachineFindWords;
  noneFound: string;
  /** On the list: the hub's row, a line's round trip, how many agents are
   *  at work on a machine, and a read-only device with what it is. */
  listHub: string;
  listMs(ms: number): string;
  busy(n: number): string;
  /** A managed machine (`world-network.md`, 受管机器): its tag, and what it
   *  is behind a pointer's rest. */
  managed: string;
  managedWhat: string;
  /** A phone signed in to the account, beside the machines: under its
   *  name, behind a pointer's rest, since when, and taking it out. */
  phone: string;
  phoneWhat: string;
  phoneSince(at: string): string;
  removePhone: string;
  removePhoneWhat(name: string): string;
  /** A visitor's machine, as the network's administrator sees it: under its
   *  name, and behind a pointer's rest. */
  visitorMachine: string;
  visitorWhat: string;
  /** After the name of a visitor's group of machines. */
  visitor: string;
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
  relay: string;
  relayTitle(machine: string): string;
  relayAddresses: string;
  relayWhat: string;
  relayGo: string;
  relayStopped(machine: string): string;
  /** Dissolving the network: the super administrator's, from beside the
   *  server on the drawing; `away`, its machines and phones offline now
   *  (`world-network.md`, 解散网络). */
  dissolve: string;
  dissolveTitle(hub: string): string;
  dissolveWhat(hub: string, away: number): string;
  dissolveGo: string;
  /** Said once on each computer or phone that was on networks dissolved. */
  dissolvedTitle(hubs: string[]): string;
  dissolvedWhat(phone: boolean, many: boolean): string;
  dissolvedSeen: string;
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
  /** Over the field the network's name is typed into, to move or dissolve it. */
  typeName(hub: string): string;
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
  formerly: (names) => `曾用名 ${names.join("、")}`,
  hubOnline: "已连接",
  hubConnecting: "连接中",
  hubDownSince: (at, why) => (why ? `${at} 起断开：${why}` : `${at} 起断开`),
  hubOff: "未连接",
  hubIs: "认出网络里的每台设备，管着谁能用哪台机器，机器之间的话经它转达",
  hubTitle: (name) => `中枢服务器 ${name}`,
  hubMachines: (n) => `${n} 台机器`,
  levelSuper: "管理员",
  levelMember: "成员",
  inviteDevice: "添加设备",
  manageHub: "管理账号",
  linkOnline: (address) => `已连上 ${address}`,
  linkVia: (through, address) => `经 ${through} 连上 ${address}`,
  linkConnecting: () => "连接中",
  linkStopped: () => "已被中枢服务器移除",
  linkDissolved: "网络已解散",
  registeredUnder: (account) => `${account} 的机器`,
  onlyLogins: (accounts) => `已登录 ${accounts}`,
  via: (through) => `经 ${through} 连着中枢服务器`,
  viaShort: (through) => `经 ${through}`,
  yours: "你的",
  ownedBy: (account) => `${account} 的`,
  sharedInShort: (owner) => `${owner} 共享`,
  superInShort: (owner) => `成员 ${owner} 的`,
  sharedIn: (owner) => `${owner} 共享给你，你和 ${owner} 一起管它、用它上面的所有智能体；你在上面的操作 ${owner} 可见`,
  superIn: (owner) => `成员 ${owner} 的机器，你作为管理员可以使用；你的操作 ${owner} 可见`,
  sharedTo: "共享给",
  assisting: "协助",
  takeBack: (to) => `收回 ${to}`,
  leave: "不再用这台",
  leaveTitle: (machine) => `不再用 ${machine}？`,
  leaveWhat: (owner) => `这台不再共享给你，要再用就请 ${owner} 共享`,
  endAssist: (to) => `结束对 ${to} 的协助`,
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
  assist: "协助",
  remove: "移除",
  assistShort: (owner) => `来自 ${owner}`,
  assistAgents: (owner) => `${owner} 的智能体`,
  assistantNote: "在你的机器上工作",
  assistantSaid: (owner) => `${owner} 的智能体在协助你`,
  assistantGone: "该登录已不可用",
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
  moreActs: "更多",
  closeSpot: "不选了",
  spot: { copy: "复制地址", address: "地址", link: "连接", machines: "机器", admin: "管理员", signedIn: "登录", state: "状态" },
  tookBack: (to) => `已收回 ${to}`,
  undo: "撤回",
  confirm: "确定",
  more: "细节",
  takesDirect: "别的机器可以直接连到这台",
  mineGroup: "我的",
  assistGroup: (owner) => `来自 ${owner}`,
  groups: {
    group: "分组",
    groupTitle: (machine) => `把 ${machine} 放进分组`,
    groupMany: (count) => `把这 ${count} 台放进分组`,
    noGroup: "不分组",
    newGroup: "新建分组",
    groupName: "组名",
    groupGo: "好",
    cancel: "取消",
    more: (group) => `${group}：更多`,
    renameGroup: "改组名",
    renameGroupTitle: (group) => `给「${group}」改个名字`,
    endGroup: "解散分组",
    movedOne: (machine, group) => (group ? `${machine} 进了「${group}」` : `${machine} 不在分组里了`),
    movedMany: (count, group) => (group ? `${count} 台进了「${group}」` : `${count} 台不在分组里了`),
    renamedGroup: (from, to) => `「${from}」改叫「${to}」了`,
    endedGroup: (group) => `「${group}」解散了`,
    groupNameLong: "组名最多 32 个字",
    groupNameEmpty: "写一个组名",
  },
  groupSub: (count, online) => `${count} 台　${online} 在线`,
  groupCount: (count) => `${count} 台`,
  sharedSection: "共享给我的",
  membersSection: "其他成员的",
  assistSection: "协助我的",
  viewMap: "图",
  viewList: "列表",
  views: "显示方式",
  find: {
    find: "查找机器",
    placeholder: "机器、人或分组…",
    found: (total) => `${total} 台`,
    count: (current, total) => `${current} / ${total}`,
    none: "无结果",
    previous: "上一个",
    next: "下一个",
    close: "关闭查找",
  },
  noneFound: "没有匹配的机器",
  listHub: "中枢服务器",
  listMs: (ms) => `${ms} ms`,
  busy: (n) => `${n} 个智能体在进行`,
  managed: "受管",
  managedWhat: "受管机器：不登录任何账号，你的智能体在上面干活",
  phone: "手机",
  phoneWhat: "手机只登录这个账号，上面不跑智能体",
  phoneSince: (at) => `${at} 登录`,
  removePhone: "移除",
  removePhoneWhat: (name) => `${name} 将退出这个账号。`,
  visitorMachine: "访客的机器",
  visitorWhat: "访客的机器只归访客自己：你看得到它在不在，用不了它",
  visitor: "访客",
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
  framedRemoveWhat: (names) => `${names.join("、")} 将断开中枢服务器并退出所有账号，共享与协助随之失效。机器上的数据不受影响。`,
  allowDirect: "允许直接连到这台",
  stopDirect: "不再允许直接连到这台",
  directTitle: (machine) => `允许直接连到 ${machine}`,
  directAddresses: "别的机器连它的地址",
  directSeen: "中枢服务器看到的",
  directCard: "网卡",
  directPort: "端口",
  directPickOne: "至少选一个地址",
  directPortWrong: "端口是 1 到 65535 之间的数",
  directWhat: "别的机器和它直接往来，不经中枢服务器，连不上中枢的机器也能经它绕过去。开启时中枢会从外面试连一次，防火墙、安全组或路由器要放行这个端口。",
  directGo: "允许",
  directChecking: "中枢服务器正在试着连它",
  directAllowed: (machine) => `已允许直接连到 ${machine}`,
  stopDirectTitle: (machine) => `不再允许直接连到 ${machine}？`,
  stopDirectSaid: "别的机器和它之间改经中枢服务器转。",
  directStopped: (machine) => `已不再允许直接连到 ${machine}`,
  relay: "替局域网里的机器连中枢",
  relayTitle: (machine) => `让 ${machine} 替局域网里的机器连中枢`,
  relayAddresses: "局域网里的机器连它的地址",
  relayWhat: "你在这个局域网里的机器连不上中枢服务器时，经它连过去，加入网络也能经它。它只转加密的连接，读不到内容；它开着，那些机器才在线。",
  relayGo: "开启",
  relayStopped: (machine) => `${machine} 已不再替局域网里的机器连中枢`,
  dissolve: "解散网络",
  dissolveTitle: (hub) => `解散 ${hub}？`,
  dissolveWhat: (hub, away) =>
    `所有机器和手机都会离开 ${hub}，入网码作废，以后谁也进不来，不能撤回。${away ? `现在不在线的 ${away} 台，下次上线时离开。` : ""}各台电脑上的 Fastctx 和数据都留着。`,
  dissolveGo: "解散",
  dissolvedTitle: (hubs) => `${hubs.join("、")} 已解散`,
  dissolvedWhat: (phone, many) => `管理员解散了${many ? "这些网络" : "这个网络"}，这台${phone ? "手机" : "电脑"}已离开。Fastctx 和这里的数据都还在。`,
  dissolvedSeen: "知道了",
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
  typeName: (hub) => `输入网络名称 ${hub} 确认`,
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
  logBy: (account, device) => [account, device].filter(Boolean).join("　"),
  logUnverified: "未验证",
  logSaid: (a) => {
    switch (a.change) {
      case "share":
        return a.shared ? `把 ${a.machine} 共享给 ${a.to}` : `收回 ${a.machine} 对 ${a.to} 的共享`;
      // Assistance, under the names the hub first wrote it under.
      case "lend":
        if (!a.lent) return `结束 ${a.machine} 上的 ${agentName(a.agent)} 对 ${a.to} 的协助`;
        return `让 ${a.machine} 上的 ${agentName(a.agent)} 协助 ${a.to}${a.until ? `，到 ${zhMoment.stamp(new Date(a.until))}` : ""}${a.daily ? `，每天 约 ${wholeDollars(a.daily / 1_000_000)}` : ""}${a.weekly ? `，每周 约 ${wholeDollars(a.weekly / 1_000_000)}` : ""}${a.five_hour ? `，5 小时额度用到 ${a.five_hour}% 停` : ""}`;
      case "lend_ended":
        return `${agentName(a.agent)} 对 ${a.to} 的协助到期结束`;
      case "plan":
        return a.plan ? `把 ${a.machine} 上 ${agentName(a.agent)} 的套餐标为 ${a.plan}` : `去掉 ${a.machine} 上 ${agentName(a.agent)} 的套餐标记`;
      case "stop_acting":
        return `撤销 ${a.machine} 访问的权限`;
      case "relays":
        return `改了 ${a.machine} 连中枢服务器的方式`;
      case "relay_at":
        return a.addresses.length ? `让 ${a.machine} 替局域网里的机器连中枢` : `${a.machine} 不再替局域网里的机器连中枢`;
      case "direct":
        return a.at.length ? `允许直接连到 ${a.machine}` : `不再允许直接连到 ${a.machine}`;
      case "remove":
        return a.then === "everything"
          ? `移除 ${a.name} 并彻底卸载`
          : a.then === "uninstall"
            ? `移除 ${a.name} 并卸载，保留数据`
            : `移除 ${a.name}`;
      case "uninstall_left":
        return `${a.name} 没卸完：${a.left}`;
      case "update_due":
        return a.version ? `${a.machine} 上线就更新到 ${a.version}` : `撤销 ${a.machine} 上线就更新`;
      case "updated":
        return `${a.by} 把 ${a.machine} 从 ${a.from} 更新到 ${a.to}`;
      case "rename":
        return `把 ${a.machine} 改名为 ${a.to}`;
      case "group":
        return a.group ? `把 ${a.machine} 放进「${a.group}」` : `让 ${a.machine} 不再分组`;
      case "rename_group":
        return a.to ? `把 ${a.owner} 的「${a.from}」改叫「${a.to}」` : `解散 ${a.owner} 的「${a.from}」`;
      case "token": {
        const named = a.label ?? a.name;
        const many = a.uses === 0 ? "，不限台数" : a.uses && a.uses > 1 ? `，${a.uses} 台` : "";
        const what = a.way === "line" || (!a.way && a.register && !a.managed) ? (a.managed ? "受管 Linux 一行命令" : "Linux 一行命令") : a.managed ? "受管机器的入网码" : a.register ? "电脑入网码" : "手机二维码";
        return `生成${what}${named ? `「${named}」` : ""}${many}`;
      }
      case "revoke_token":
        return `作废${a.way === "scan" ? "手机二维码" : a.way === "line" ? "Linux 一行命令" : a.managed ? "受管机器的入网码" : "入网码"}${a.label ? `「${a.label}」` : ""}`;
      case "joined":
        if (a.managed) return `${a.name} 作为受管机器加入`;
        return a.registered ? `${a.name} 加入` : `${a.name} 登录`;
      case "left":
        return `${a.name} 离开`;
      case "allow_acting":
        return `${a.machine} 上的 ${agentName(a.agent)} 获准访问`;
      case "allow_sentinel_home":
        return `${a.machine} 上的哨兵获准访问`;
      case "acting_stopped":
        return `${a.machine} 不再能替 ${a.account} 访问`;
      case "forget":
        return `删除 ${a.conversation.machine} 上的对话 ${a.conversation.resident}`;
      case "forgotten":
        return `${a.on} 上删除 ${a.conversation.resident}`;
      case "dissolve":
        return `解散网络 ${a.name}`;
      case "updates":
        if (a.intake !== undefined) return `中枢取新版本每秒至多 ${megabytes(a.intake)} MB`;
        return a.managed_auto === undefined
          ? `网络的更新改为${a.channel === "beta" ? "测试版" : "正式版"}${a.minimum ? `，至少 ${a.minimum}` : ""}`
          : `网络的更新改为${a.minimum ? `至少 ${a.minimum}` : "不要求最低版本"}，受管机器${a.managed_auto ? "自动更新" : "不自动更新"}`;
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
  formerly: (names) => `formerly ${names.join(", ")}`,
  hubOnline: "connected",
  hubConnecting: "connecting",
  hubDownSince: (at, why) => (why ? `down since ${at}: ${why}` : `down since ${at}`),
  hubOff: "not connected",
  hubIs: "It knows every device in the network, keeps who may use which machine, and passes on what the machines say to each other",
  hubTitle: (name) => `Hub ${name}`,
  hubMachines: (n) => `${n} machine${n === 1 ? "" : "s"}`,
  levelSuper: "administrator",
  levelMember: "member",
  inviteDevice: "Add device",
  manageHub: "Accounts",
  linkOnline: (address) => `connected to ${address}`,
  linkVia: (through, address) => `connected to ${address} through ${through}`,
  linkConnecting: () => "connecting",
  linkStopped: () => "removed by the hub",
  linkDissolved: "network dissolved",
  registeredUnder: (account) => `${account}'s machine`,
  onlyLogins: (accounts) => `signed in to ${accounts}`,
  via: (through) => `reaches the hub through ${through}`,
  viaShort: (through) => `via ${through}`,
  yours: "yours",
  ownedBy: (account) => `${account}'s`,
  sharedInShort: (owner) => `shared by ${owner}`,
  superInShort: (owner) => `member ${owner}'s`,
  sharedIn: (owner) => `Shared with you by ${owner}: you manage it with ${owner} and use all its agents; ${owner} can see what you do on it`,
  superIn: (owner) => `Member ${owner}'s machine, available to you as administrator; ${owner} can see what you do on it`,
  sharedTo: "Shared with",
  assisting: "Assisting",
  takeBack: (to) => `Revoke ${to}`,
  leave: "Stop using it",
  leaveTitle: (machine) => `Stop using ${machine}?`,
  leaveWhat: (owner) => `It is no longer shared with you; ask ${owner} to share it again`,
  endAssist: (to) => `Stop assisting ${to}`,
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
  assist: "Assist",
  remove: "Remove",
  assistShort: (owner) => `from ${owner}`,
  assistAgents: (owner) => `${owner}'s agents`,
  assistantNote: "Works on your machines",
  assistantSaid: (owner) => `${owner}'s agent is assisting you`,
  assistantGone: "Login unavailable",
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
  moreActs: "More",
  closeSpot: "Deselect",
  spot: { copy: "Copy address", address: "Address", link: "Connection", machines: "Machines", admin: "Admin", signedIn: "Signed in", state: "Status" },
  tookBack: (to) => `Revoked ${to}`,
  undo: "Undo",
  confirm: "Confirm",
  more: "Details",
  takesDirect: "Other machines may connect to it directly",
  mineGroup: "Mine",
  assistGroup: (owner) => `From ${owner}`,
  groups: {
    group: "Group",
    groupTitle: (machine) => `Put ${machine} in a group`,
    groupMany: (count) => `Put these ${count} in a group`,
    noGroup: "No group",
    newGroup: "New group",
    groupName: "Group name",
    groupGo: "OK",
    cancel: "Cancel",
    more: (group) => `More for ${group}`,
    renameGroup: "Rename group",
    renameGroupTitle: (group) => `Rename “${group}”`,
    endGroup: "Ungroup",
    movedOne: (machine, group) => (group ? `${machine} is in “${group}”` : `${machine} is in no group`),
    movedMany: (count, group) => (group ? `${count} machines are in “${group}”` : `${count} machines are in no group`),
    renamedGroup: (from, to) => `“${from}” is “${to}” now`,
    endedGroup: (group) => `“${group}” is ungrouped`,
    groupNameLong: "A group's name is at most 32 characters",
    groupNameEmpty: "Give the group a name",
  },
  groupSub: (count, online) => `${count}, ${online} online`,
  groupCount: (count) => String(count),
  sharedSection: "Shared with me",
  membersSection: "Other members'",
  assistSection: "Assisting me",
  viewMap: "Map",
  viewList: "List",
  views: "Show as",
  find: {
    find: "Find machines",
    placeholder: "Machine, person or group…",
    found: (total) => `${total} machine${total === 1 ? "" : "s"}`,
    count: (current, total) => `${current} of ${total}`,
    none: "No matches",
    previous: "Previous",
    next: "Next",
    close: "Close find",
  },
  noneFound: "No machine matches",
  listHub: "Hub",
  listMs: (ms) => `${ms} ms`,
  busy: (n) => `${n} agent${n === 1 ? "" : "s"} at work`,
  managed: "Managed",
  managedWhat: "Managed machine: logged in to no account, your agents work on it",
  phone: "Phone",
  phoneWhat: "A phone only signs in to the account; no agent runs on it",
  phoneSince: (at) => `signed in ${at}`,
  removePhone: "Remove",
  removePhoneWhat: (name) => `${name} will be signed out of this account.`,
  visitorMachine: "a visitor's",
  visitorWhat: "A visitor's machine is the visitor's alone: you see whether it is there, and cannot use it",
  visitor: "visitor",
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
  framedRemoveWhat: (names) => `${names.join(", ")} will leave the hub and every account; their shares and assistance end. Nothing on the machines is touched.`,
  allowDirect: "Allow direct connections",
  stopDirect: "Stop direct connections",
  directTitle: (machine) => `Allow direct connections to ${machine}`,
  directAddresses: "Where others connect to it",
  directSeen: "as the hub sees it",
  directCard: "network card",
  directPort: "Port",
  directPickOne: "Choose at least one address",
  directPortWrong: "A port is a number from 1 to 65535",
  directWhat: "Other machines reach it directly, not through the hub, and one cut off from the hub can go through it. The hub tries it once from outside; allow the port in the firewall, security group or router.",
  directGo: "Allow",
  directChecking: "The hub is trying to reach it",
  directAllowed: (machine) => `Allowed direct connections to ${machine}`,
  stopDirectTitle: (machine) => `Stop direct connections to ${machine}?`,
  stopDirectSaid: "What goes between it and other machines will go through the hub.",
  directStopped: (machine) => `No more direct connections to ${machine}`,
  relay: "Relay for machines on this network",
  relayTitle: (machine) => `Let ${machine} relay for machines on its network`,
  relayAddresses: "Where machines on its network reach it",
  relayWhat: "Your machines on this network that cannot reach the hub reach it through this one, joining too. It carries their encrypted connection and reads nothing of it; they are online while it is.",
  relayGo: "Turn on",
  relayStopped: (machine) => `${machine} no longer relays for machines on its network`,
  dissolve: "Dissolve network",
  dissolveTitle: (hub) => `Dissolve ${hub}?`,
  dissolveWhat: (hub, away) =>
    `Every machine and phone leaves ${hub}, its codes stop working, and no one can join it again. This can't be undone.${away ? ` The ${away} offline now leave when they're next online.` : ""} Fastctx and its data stay on every computer.`,
  dissolveGo: "Dissolve",
  dissolvedTitle: (hubs) => `${hubs.join(", ")} ${hubs.length > 1 ? "were" : "was"} dissolved`,
  dissolvedWhat: (phone, many) => `${many ? "Their administrators dissolved them" : "Its administrator dissolved it"}, and this ${phone ? "phone" : "computer"} has left. Fastctx and your data here are untouched.`,
  dissolvedSeen: "OK",
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
  typeName: (hub) => `Type ${hub} to confirm`,
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
  logBy: (account, device) => [account, device].filter(Boolean).join(", "),
  logUnverified: "unverified",
  logSaid: (a) => {
    switch (a.change) {
      case "share":
        return a.shared ? `Shared ${a.machine} with ${a.to}` : `Stopped sharing ${a.machine} with ${a.to}`;
      // Assistance, under the names the hub first wrote it under.
      case "lend":
        if (!a.lent) return `Ended the assistance of ${agentName(a.agent)} on ${a.machine} to ${a.to}`;
        return `Let ${agentName(a.agent)} on ${a.machine} assist ${a.to}${a.until ? ` until ${enMoment.stamp(new Date(a.until))}` : ""}${a.daily ? `, ≈ ${wholeDollars(a.daily / 1_000_000)} a day` : ""}${a.weekly ? `, ≈ ${wholeDollars(a.weekly / 1_000_000)} a week` : ""}${a.five_hour ? `, stopping at ${a.five_hour}% of the 5-hour limit` : ""}`;
      case "lend_ended":
        return `The assistance of ${agentName(a.agent)} to ${a.to} reached its end`;
      case "plan":
        return a.plan ? `Marked ${agentName(a.agent)} on ${a.machine} as ${a.plan}` : `Cleared the plan of ${agentName(a.agent)} on ${a.machine}`;
      case "stop_acting":
        return `Revoked ${a.machine}'s access`;
      case "relays":
        return `Changed how ${a.machine} reaches the hub`;
      case "relay_at":
        return a.addresses.length ? `Let ${a.machine} relay for machines on its network` : `${a.machine} no longer relays for machines on its network`;
      case "direct":
        return a.at.length ? `Allowed direct connections to ${a.machine}` : `No more direct connections to ${a.machine}`;
      case "remove":
        return a.then === "everything"
          ? `Removed ${a.name} and everything Fastctx wrote there`
          : a.then === "uninstall"
            ? `Removed ${a.name} and uninstalled Fastctx, keeping its data`
            : `Removed ${a.name}`;
      case "uninstall_left":
        return `${a.name} did not finish uninstalling: ${a.left}`;
      case "update_due":
        return a.version ? `${a.machine} updates to ${a.version} once it is back` : `${a.machine} no longer updates once it is back`;
      case "updated":
        return `${a.by} updated ${a.machine} from ${a.from} to ${a.to}`;
      case "rename":
        return `Renamed ${a.machine} to ${a.to}`;
      case "group":
        return a.group ? `Put ${a.machine} in “${a.group}”` : `Took ${a.machine} out of its group`;
      case "rename_group":
        return a.to ? `Renamed ${a.owner}'s “${a.from}” to “${a.to}”` : `Ungrouped ${a.owner}'s “${a.from}”`;
      case "token": {
        const named = a.label ?? a.name;
        const many = a.uses === 0 ? ", any number" : a.uses && a.uses > 1 ? `, ${a.uses} devices` : "";
        const what = a.way === "line" || (!a.way && a.register && !a.managed) ? (a.managed ? "a managed Linux command" : "a Linux command") : a.managed ? "a code for managed machines" : a.register ? "a computer code" : "a sign-in QR code";
        return `Made ${what}${named ? ` “${named}”` : ""}${many}`;
      }
      case "revoke_token":
        return `Revoked ${a.way === "scan" ? "a sign-in QR code" : a.way === "line" ? "a Linux command" : a.managed ? "a code for managed machines" : "a code"}${a.label ? ` “${a.label}”` : ""}`;
      case "joined":
        if (a.managed) return `${a.name} joined as a managed machine`;
        return a.registered ? `${a.name} joined` : `${a.name} signed in`;
      case "left":
        return `${a.name} left`;
      case "allow_acting":
        return `${agentName(a.agent)} on ${a.machine} was allowed access`;
      case "allow_sentinel_home":
        return `Sentinels on ${a.machine} were allowed access`;
      case "acting_stopped":
        return `${a.machine} no longer acts for ${a.account}`;
      case "forget":
        return `Deleted conversation ${a.conversation.resident} on ${a.conversation.machine}`;
      case "forgotten":
        return `${a.conversation.resident} deleted on ${a.on}`;
      case "dissolve":
        return `Dissolved the network ${a.name}`;
      case "updates":
        if (a.intake !== undefined) return `The hub takes new releases in at ${megabytes(a.intake)} MB a second at most`;
        return a.managed_auto === undefined
          ? `Network updates set to ${a.channel === "beta" ? "beta" : "stable"}${a.minimum ? `, at least ${a.minimum}` : ""}`
          : `Network updates set to ${a.minimum ? `at least ${a.minimum}` : "no minimum"}; managed machines ${a.managed_auto ? "update by themselves" : "do not update by themselves"}`;
      default:
        return null;
    }
  },
};

/** The sheets on a machine: whom its owner shares it with, whom one login
 *  of an agent on it assists, and removing it. */
export interface AccessWords {
  shareTitle(machine: string): string;
  shareWhat: string;
  shareSees: string;
  sharedTo: string;
  shareGo: string;
  assistTitle(login: string, machine: string): string;
  assistWhat: string;
  assistSees: string;
  assisting: string;
  assistGo: string;
  /** The assist sheet's own: whom it assists, until when, changing it and
   *  ending it. */
  assistWho: string;
  /** The accounts listed under the name as it is typed. */
  assistNames: string;
  assistUntil: string;
  endAssist: string;
  changeUntil: string;
  untilPassed: string;
  /** The weekly and daily allowances, Claude Code's only: amounts in
   *  dollars. */
  weekly: string;
  weeklyHint: string;
  weeklyNone: string;
  weeklyOf(amount: string): string;
  weeklyBad: string;
  /** The Fable limit that comes with a weekly one, half of it, under the
   *  weekly field. */
  fableOf(amount: string): string;
  fableHint: string;
  daily: string;
  dailyHint: string;
  dailyOf(amount: string): string;
  dailyBad: string;
  /** Neither allowance set. */
  noAllowance: string;
  /** The share of the login's own five-hour window past which it assists
   *  no one, for the whole login. */
  fiveHour: string;
  fiveHourHint: string;
  fiveHourNone: string;
  fiveHourOf(percent: number): string;
  fiveHourBad: string;
  /** The window as last read, on hover over its bar: how much is used
   *  and when it starts again, short. */
  fiveHourRead(percent: number, resets: string | null): string;
  fiveHourUnread: string;
  fiveHourReset: string;
  /** The most turns the login runs at once, for the whole login. */
  turns: string;
  turnsHint: string;
  /** None set: Fastctx follows the vendor's rate limits. */
  turnsNone: string;
  turnsOf(turns: number): string;
  turnsUnit: string;
  turnsBad: string;
  /** The most jobs an account a machine is shared with runs there at once. */
  jobsLabel: string;
  jobsHint: string;
  jobsNone: string;
  jobsOf(jobs: number): string;
  jobsUnit: string;
  jobsBad: string;
  change: string;
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
  shareSees: "对方能以这台机器的系统用户做任何事，读得到上面的文件和智能体登录的凭据。请只共享给信任的人。",
  sharedTo: "已共享给",
  shareGo: "共享",
  assistTitle: (login, machine) => `让 ${machine} 上的 ${login} 协助他人`,
  assistWhat: "只用这个登录协助对方，不共享机器。",
  assistSees: "会使用这个登录的订阅额度，运行过程在你的机器上可见、可停止。",
  assisting: "正在协助",
  assistGo: "协助",
  assistWho: "协助谁",
  assistNames: "对得上的账号",
  assistUntil: "截止",
  endAssist: "结束",
  changeUntil: "改截止时间",
  untilPassed: "这个截止时间已经过了",
  weekly: "每周上限",
  weeklyHint: "约 $ 按 API 标价折算；到了就停，和这个登录的 7 天额度一起重置。空着是不限。",
  weeklyNone: "不限",
  weeklyOf: (amount) => `每周 约 ${amount}`,
  fableOf: (amount) => `其中 Fable 至多 ${amount}`,
  fableHint: "Fable 的上限自动是每周上限的一半，不能另填。",
  weeklyBad: "每周上限要填一个大于 0 的金额",
  daily: "每天上限",
  dailyHint: "约 $ 按 API 标价折算；按最近 24 小时算，到了就停。空着是不限。",
  dailyOf: (amount) => `每天 约 ${amount}`,
  dailyBad: "每天上限要填一个大于 0 的金额",
  noAllowance: "不限额",
  fiveHour: "5 小时额度",
  fiveHourHint: "这个登录自己的 5 小时额度用到这个比例，就暂停协助所有人，你自己的用量也算在内。",
  fiveHourNone: "不限",
  fiveHourOf: (percent) => `用到 ${percent}% 停`,
  fiveHourBad: "填 1 到 100 之间的整数",
  fiveHourRead: (percent, resets) => `已用 ${percent}%${resets ? `，${resets} 重置` : ""}`,
  fiveHourUnread: "还没读到这个登录的 5 小时额度",
  fiveHourReset: "已重置",
  turns: "同时几轮",
  turnsHint: "这个登录同时最多开几轮，你自己的也算；满了就排队，你的先开。不填则按厂商的限速自动收放。",
  turnsNone: "自动",
  turnsOf: (turns) => `至多 ${turns} 轮`,
  turnsUnit: "轮",
  turnsBad: "填 1 以上的整数",
  jobsLabel: "同时几条作业",
  jobsHint: "这个账号在这台上同时最多跑几条作业；到了，要等一条结束才能再起。不填为不限。",
  jobsNone: "作业不限",
  jobsOf: (jobs) => `至多 ${jobs} 条作业`,
  jobsUnit: "条",
  jobsBad: "填 1 以上的整数",
  change: "改",
  save: "保存",
  cancel: "取消",
  when: zhWhen,
  moment: zhMoment,
  nobodyYet: "暂无",
  to: "账号",
  toHint: "对方的账号名",
  takeBack: "收回",
  removeTitle: (machine) => `移除 ${machine}？`,
  removeWhat: (machine) => `${machine} 将断开中枢服务器并退出所有账号，共享与协助随之失效。机器上的数据不受影响。`,
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
  shareSees: "They can do anything this machine's system user can, its files and its agents' login credentials included. Share only with people you trust.",
  sharedTo: "Shared with",
  shareGo: "Share",
  assistTitle: (login, machine) => `Let ${login} on ${machine} assist others`,
  assistWhat: "Only this login assists them, not the machine.",
  assistSees: "It uses this login's subscription. You can see and stop it on your machine.",
  assisting: "Assisting",
  assistGo: "Assist",
  assistWho: "Assist",
  assistNames: "Matching accounts",
  assistUntil: "Ends",
  endAssist: "End",
  changeUntil: "Change when it ends",
  untilPassed: "That time has already passed",
  weekly: "Weekly limit",
  weeklyHint: "Dollars at the API's list prices; reaching it stops them until this login's 7-day limit resets. Empty is no limit.",
  weeklyNone: "No limit",
  weeklyOf: (amount) => `≈ ${amount} a week`,
  fableOf: (amount) => `Fable: up to ${amount} of it`,
  fableHint: "Fable gets half the weekly limit, set for you.",
  weeklyBad: "The weekly limit needs an amount above 0",
  daily: "Daily limit",
  dailyHint: "Dollars at the API's list prices over the last 24 hours; reaching it stops them. Empty is no limit.",
  dailyOf: (amount) => `≈ ${amount} a day`,
  dailyBad: "The daily limit needs an amount above 0",
  noAllowance: "No limit",
  fiveHour: "5-hour limit",
  fiveHourHint: "Once this login's own 5-hour limit is used to this share, it stops assisting anyone. Your own use counts too.",
  fiveHourNone: "No limit",
  fiveHourOf: (percent) => `Stop at ${percent}%`,
  fiveHourBad: "Enter a whole number from 1 to 100",
  fiveHourRead: (percent, resets) => `${percent}% used${resets ? `, resets ${resets}` : ""}`,
  fiveHourUnread: "This login's 5-hour limit hasn't been read yet",
  fiveHourReset: "Reset",
  turns: "Turns at once",
  turnsHint: "The most turns this login runs at once, yours included; past it they queue, yours first. Left empty, it follows the vendor's rate limits.",
  turnsNone: "Auto",
  turnsOf: (turns) => `At most ${turns}`,
  turnsUnit: "turns",
  turnsBad: "Enter a whole number from 1",
  jobsLabel: "Jobs at once",
  jobsHint: "The most jobs this account runs on this machine at once; past it, one has to end before another starts. Left empty, no limit.",
  jobsNone: "Any jobs",
  jobsOf: (jobs) => `At most ${jobs} jobs`,
  jobsUnit: "jobs",
  jobsBad: "Enter a whole number from 1",
  change: "Change",
  save: "Save",
  cancel: "Cancel",
  when: enWhen,
  moment: enMoment,
  nobodyYet: "None yet",
  to: "Account",
  toHint: "Their account name",
  takeBack: "Revoke",
  removeTitle: (machine) => `Remove ${machine}?`,
  removeWhat: (machine) => `${machine} will disconnect from the hub and sign out of every account; its shares and assistance end. Data on it is untouched.`,
  removeGo: "Remove",
  reason: "Reason",
  reasonDefault: "removed",
  working: "Working…",
  close: "Close",
  failed: failure(enProblems, "Couldn't complete"),
};

/** The sheet that adds another device to an account. */
export interface TokenWords {
  title: string;
  kinds: Record<"computer" | "phone" | "server" | "managed", string>;
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
  /** How long a code may stay valid: an hour, a day, a week, thirty days,
   *  or as long as it is not revoked. */
  spans: [string, string, string, string, string];
  makeCode: string;
  makeCommand: string;
  making: string;
  qr: string;
  command: string;
  copy: string;
  copied: string;
  again: string;
  close: string;
  problems: ProblemWords;
  /** A code (`world-network.md`, 入网码): its name, how many devices it
   *  lets in, for how long, and once made, how long and for how many; for
   *  managed machines, the three steps of using it and the line. A Linux
   *  line for several hosts names their group. */
  label: string;
  labelHint: string;
  lineHint: string;
  uses: string;
  usesOne: string;
  usesAny: string;
  usesSome: string;
  unit: string;
  usesSaid(most: number): string;
  makeManaged: string;
  codeUntil(at: string, uses: number): string;
  codeLasting(uses: number): string;
  /** The codes made that still let devices in, beside the title. */
  codes: string;
  codesNone: string;
  /** Three steps under drawings; in Chinese a zero-width space marks
   *  where a line may wrap, between phrases, never inside one. */
  steps: [string, string, string];
  /** How each way in is used, in three steps each, under its drawings. */
  joinSteps: Record<"computer" | "phone" | "line" | "windows" | "mac", [string, string, string]>;
  line: string;
  /** A Linux host as a managed machine: the switch, what it means behind a
   *  pointer's rest, and why it cannot be had. */
  managed: string;
  managedWhat: string;
  managedOld: string;
  /** How a managed machine comes in, and what its Windows or Mac line is
   *  run as. */
  managedWays: { code: string; windows: string; mac: string };
  windowsCommand: string;
  macCommand: string;
}

/** How many devices a code lets in, as its line under it says. */
/** A ceiling in bytes a second, as megabytes a second. */
const megabytes = (rate: number) => {
  const value = rate / 1_000_000;
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
};

const zhUses = (uses: number) => (uses === 0 ? "不限台数" : uses === 1 ? "只能进 1 台" : `能进 ${uses} 台`);
const enUses = (uses: number) => (uses === 0 ? "any number of devices" : uses === 1 ? "one device" : `${uses} devices`);

export const zhToken: TokenWords = {
  title: "添加设备",
  kinds: { computer: "电脑", phone: "手机", server: "Linux 主机", managed: "受管机器" },
  ways: { ssh: "经 SSH", line: "一行命令" },
  managedWays: { code: "入网码", windows: "Windows", mac: "Mac" },
  windowsCommand: "以管理员身份运行",
  macCommand: "以 root 身份运行",
  added: (machine) => `${machine} 已加入`,
  hub: "中枢服务器地址",
  account: "账号",
  name: "主机名称",
  nameAuto: "自动",
  nameSaid: zhNameSaid,
  valid: "有效期",
  spans: ["1 小时", "1 天", "7 天", "30 天", "不过期"],
  makeCode: "生成二维码",
  makeCommand: "生成命令",
  making: "正在生成…",
  qr: "登录二维码",
  command: "在主机终端中运行",
  copy: "复制",
  copied: "已复制",
  again: "重新生成",
  close: "关闭",
  problems: zhProblems,
  label: "名字",
  labelHint: "如：机房 A",
  lineHint: "如：GPU 集群",
  uses: "能进几台",
  usesOne: "1 台",
  usesAny: "不限",
  usesSome: "自定",
  unit: "台",
  usesSaid: (most) => `填 1 到 ${most} 之间的台数`,
  makeManaged: "生成入网码",
  codeUntil: (at, uses) => `${at} 前有效，${zhUses(uses)}`,
  codeLasting: (uses) => `不过期，${zhUses(uses)}`,
  codes: "入网码",
  codesNone: "没有还能用的入网码",
  steps: ["换到使用者\u200b不登录的\u200b系统账户", "装好 Fastctx，\u200b打开", "在「加入网络」里\u200b粘贴"],
  joinSteps: {
    computer: ["在那台电脑上\u200b装好 Fastctx，\u200b打开", "点「加入网络」", "填上面的\u200b地址和账号，\u200b输入密码"],
    phone: ["在手机上\u200b装好 Fastctx，\u200b打开", "点「加入网络」", "扫\u200b这里生成的码"],
    line: ["生成\u200b下面这一行", "在服务器的终端里\u200b粘贴运行", "它装好 Fastctx，\u200b加入网络"],
    windows: ["生成\u200b下面这一行", "以管理员身份\u200b打开命令提示符，\u200b粘贴运行", "它装好 Fastctx，\u200b成为受管机器"],
    mac: ["生成\u200b下面这一行", "以\u00a0root\u00a0身份\u200b打开终端，\u200b粘贴运行", "它装好 Fastctx，\u200b成为受管机器"],
  },
  line: "或在那个账户的终端里运行",
  managed: "受管机器",
  managedWhat: "它不登录任何账号，只归你：你的智能体在上面干活，它自己什么都用不了",
  managedOld: "中枢服务器的 Fastctx 版本太旧，先更新它",
};

export const enToken: TokenWords = {
  title: "Add a device",
  kinds: { computer: "Computer", phone: "Phone", server: "Linux host", managed: "Managed" },
  ways: { ssh: "Over SSH", line: "One command" },
  managedWays: { code: "Code", windows: "Windows", mac: "Mac" },
  windowsCommand: "Run as administrator",
  macCommand: "Run as root",
  added: (machine) => `${machine} joined`,
  hub: "Hub address",
  account: "Account",
  name: "Host name",
  nameAuto: "automatic",
  nameSaid: enNameSaid,
  valid: "Valid for",
  spans: ["1 hour", "1 day", "7 days", "30 days", "Never expires"],
  makeCode: "Create QR code",
  makeCommand: "Create command",
  making: "Creating…",
  qr: "Login QR code",
  command: "Run in the host's terminal",
  copy: "Copy",
  copied: "Copied",
  again: "Create again",
  close: "Close",
  problems: enProblems,
  label: "Name",
  labelHint: "e.g. Lab A",
  lineHint: "e.g. GPU cluster",
  uses: "How many devices",
  usesOne: "One",
  usesAny: "Any",
  usesSome: "Custom",
  unit: "",
  usesSaid: (most) => `Give a number from 1 to ${most}`,
  makeManaged: "Create code",
  codeUntil: (at, uses) => `Valid until ${at}, ${enUses(uses)}`,
  codeLasting: (uses) => `Never expires, ${enUses(uses)}`,
  codes: "Codes",
  codesNone: "No code lets a device in now",
  steps: ["Sign in as a system account no one at it uses", "Install Fastctx and open it", "Paste it in Join a network"],
  joinSteps: {
    computer: ["Install Fastctx on that computer and open it", "Choose Join a network", "Enter the address and account above, and the password"],
    phone: ["Install Fastctx on the phone and open it", "Choose Join a network", "Scan the code made here"],
    line: ["Make the line below", "Paste it in the server's terminal and run it", "It installs Fastctx and joins"],
    windows: ["Make the line below", "Paste it in a Command Prompt run as administrator", "It installs Fastctx as a managed machine"],
    mac: ["Make the line below", "Paste it in a Terminal run as root", "It installs Fastctx as a managed machine"],
  },
  line: "Or run in that account's terminal",
  managed: "Managed machine",
  managedWhat: "Logged in to no account, it is yours alone: your agents work on it, and it uses nothing itself",
  managedOld: "The hub runs an older Fastctx; update it first",
};
