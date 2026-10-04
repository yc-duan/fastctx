// What the frame around every page says: the sidebar, the new conversation,
// the settings. In the user's words (`world-gui.md`, 用词).

import type { AskWords } from "../conversation/asks";

export interface ShellWords {
  app: string;
  world: string;
  projects: string;
  outside: string;
  machines: string;
  settings: string;
  newTalk: string;
  newProject: string;
  edit: string;
  openSidebar: string;
  foldSidebar: string;
  /** "Claude · laptop". */
  someone(agent: string, machine: string): string;
  colleagueFor(asker: string): string;
  busy(lasted: string): string;
  idle: string;
  waiting: string;
  asleep: string;
  offline: string;
  noProjects: string;
  nobodyHere: string;
  /** The World on this machine could not be asked. */
  unreachable: string;
  thisOne: string;
  joinOrFound: string;
  more(n: number): string;
  span(ms: number): string;
}

export interface NewTalkWords {
  title: string;
  machine: string;
  agent: string;
  project: string;
  noProject: string;
  here: string;
  offline: string;
  noMachine: string;
  firstWords: string;
  hint: string;
  start: string;
  starting: string;
  cancel: string;
  notOpened(why: string): string;
}

export const zhNewTalk: NewTalkWords = {
  title: "新对话",
  machine: "在哪台机器",
  agent: "用哪一位",
  project: "进哪个项目",
  noProject: "不进项目",
  here: "这台",
  offline: "离线",
  noMachine: "没有在线的机器",
  firstWords: "对它说的第一句话…",
  hint: "Enter 开始 · Shift+Enter 换行",
  start: "开始",
  starting: "正在请它进来…",
  cancel: "关闭",
  notOpened: (why) => `没能请它进来：${why}`,
};

export const enNewTalk: NewTalkWords = {
  title: "New conversation",
  machine: "On which machine",
  agent: "With whom",
  project: "In which project",
  noProject: "No project",
  here: "this one",
  offline: "offline",
  noMachine: "No machine is online",
  firstWords: "The first thing to say…",
  hint: "Enter to begin · Shift+Enter for a new line",
  start: "Begin",
  starting: "Asking it in…",
  cancel: "Close",
  notOpened: (why) => `Could not ask it in: ${why}`,
};

export interface SettingsWords {
  title: string;
  look: string;
  theme: string;
  system: string;
  themes: Record<"warm-black" | "ink-black" | "warm-white", string>;
  language: string;
  textSize: string;
  sizes: Record<string, string>;
  device: string;
  machine: string;
  world: string;
  alone: string;
  link: string;
  version: string;
  online(hub: string, since: string): string;
  connecting(error: string | null): string;
  stopped(reason: string): string;
  notInShell: string;
}

export const zhSettings: SettingsWords = {
  title: "设置",
  look: "外观",
  theme: "主题",
  system: "跟随系统",
  themes: { "warm-black": "暖黑", "ink-black": "墨黑", "warm-white": "暖白" },
  language: "语言",
  textSize: "正文字号",
  sizes: { "14": "小", "15": "标准", "16": "大", "17": "更大" },
  device: "这台设备",
  machine: "机器名",
  world: "世界",
  alone: "独自一台",
  link: "与中枢",
  version: "版本",
  online: (hub, since) => `连着 ${hub}，从 ${since} 起`,
  connecting: (error) => (error ? `正在连，上一次没连上：${error}` : "正在连"),
  stopped: (reason) => `没有在连：${reason}`,
  notInShell: "在浏览器里看的是样例，没有连着世界。",
};

export const enSettings: SettingsWords = {
  title: "Settings",
  look: "Look",
  theme: "Theme",
  system: "As the system",
  themes: { "warm-black": "Warm black", "ink-black": "Ink black", "warm-white": "Warm white" },
  language: "Language",
  textSize: "Text size",
  sizes: { "14": "Small", "15": "Standard", "16": "Large", "17": "Larger" },
  device: "This device",
  machine: "Machine",
  world: "World",
  alone: "On its own",
  link: "To the hub",
  version: "Version",
  online: (hub, since) => `connected to ${hub} since ${since}`,
  connecting: (error) => (error ? `connecting; the last try failed: ${error}` : "connecting"),
  stopped: (reason) => `not connecting: ${reason}`,
  notInShell: "In a browser this is a specimen, not connected to the World.",
};

export interface MachinesWords {
  title: string;
  here: string;
  online(since: string): string;
  offline(since: string | null): string;
  unknown: string;
  revoked(by: string, reason: string): string;
  factsUnknown(why: string): string;
  agents: string;
  nobody: string;
  jobs(n: number): string;
  home: string;
  via(route: string): string;
}

export const zhMachines: MachinesWords = {
  title: "机器",
  here: "这台",
  online: (since) => `在线，${since}起`,
  offline: (since) => (since ? `离线，${since}起` : "离线"),
  unknown: "不知道在不在线",
  revoked: (by, reason) => `已被 ${by} 吊销：${reason}`,
  factsUnknown: (why) => `问不到它的情况：${why}`,
  agents: "上面的智能体",
  nobody: "此刻没有智能体",
  jobs: (n) => `${n} 个作业在跑`,
  home: "家目录",
  via: (route) => `经${route}连着中枢`,
};

export const enMachines: MachinesWords = {
  title: "Machines",
  here: "this one",
  online: (since) => `online since ${since}`,
  offline: (since) => (since ? `offline since ${since}` : "offline"),
  unknown: "not known whether online",
  revoked: (by, reason) => `revoked by ${by}: ${reason}`,
  factsUnknown: (why) => `could not be asked: ${why}`,
  agents: "Agents on it",
  nobody: "No agent right now",
  jobs: (n) => `${n} job${n === 1 ? "" : "s"} running`,
  home: "Home",
  via: (route) => `reaches the hub ${route}`,
};

export interface DetailsWords {
  open: string;
  close: string;
  model: string;
  modelSelf: string;
  machine: string;
  host: string;
  startedByWorld: string;
  origin: string;
  openedByYou: string;
  askedBy(asker: string, label: string): string;
  here: string;
  since(ago: string): string;
  state: string;
  busy(lasted: string): string;
  idle(ago: string): string;
  asleep(ago: string): string;
  waiting: string;
  trouble: string;
  task: string;
  colleagues: string;
  running: string;
  actions(shown: number, total: number): string;
  noActions: string;
  answers: string;
  records: string;
  stop: string;
  stopped: string;
  notRunning: string;
  cannotLook(why: string): string;
  notFound: string;
}

export const zhDetails: DetailsWords = {
  open: "关于它",
  close: "收起",
  model: "模型",
  modelSelf: "（自报）",
  machine: "在哪台机器",
  host: "程序",
  startedByWorld: "世界拉起的后台会话",
  origin: "来历",
  openedByYou: "你开启的",
  askedBy: (asker, label) => `受 ${asker} 之托：${label}`,
  here: "来了",
  since: (ago) => `${ago}前`,
  state: "此刻",
  busy: (lasted) => `在干活，已 ${lasted}`,
  idle: (ago) => `空闲，${ago}前说完`,
  asleep: (ago) => `睡着了，${ago}前`,
  waiting: "在等你回答",
  trouble: "出过的状况",
  task: "它接的活",
  colleagues: "它请的同事",
  running: "它起的、还在跑的",
  actions: (shown, total) => (shown < total ? `最近的动作（${total} 个里的最后 ${shown} 个）` : "它的动作"),
  noActions: "还没有动作。",
  answers: "交回过的",
  records: "记录在",
  stop: "停下这一轮",
  stopped: "已请它停下",
  notRunning: "它此刻没有在跑的一轮",
  cannotLook: (why) => `问不到它的详情：${why}`,
  notFound: "世界里没有找到它。",
};

export const enDetails: DetailsWords = {
  open: "About it",
  close: "Close",
  model: "Model",
  modelSelf: " (self-reported)",
  machine: "Machine",
  host: "Program",
  startedByWorld: "a background session the World started",
  origin: "Came",
  openedByYou: "opened by you",
  askedBy: (asker, label) => `for ${asker}: ${label}`,
  here: "Here",
  since: (ago) => `${ago} ago`,
  state: "Now",
  busy: (lasted) => `working, for ${lasted}`,
  idle: (ago) => `idle, finished ${ago} ago`,
  asleep: (ago) => `asleep, since ${ago} ago`,
  waiting: "waiting for your answer",
  trouble: "Trouble",
  task: "Its task",
  colleagues: "Colleagues it asked",
  running: "Still running, started by it",
  actions: (shown, total) => (shown < total ? `Latest actions (last ${shown} of ${total})` : "Its actions"),
  noActions: "No actions yet.",
  answers: "What it handed back",
  records: "Records",
  stop: "Stop this turn",
  stopped: "Asked to stop",
  notRunning: "It is running no turn now",
  cannotLook: (why) => `Its details could not be asked: ${why}`,
  notFound: "It was not found in the World.",
};

export interface ProjectFormWords {
  newTitle: string;
  editTitle(name: string): string;
  name: string;
  namePlaceholder: string;
  slug: string;
  slugHint: string;
  what: string;
  whatPlaceholder: string;
  machines: string;
  places: string;
  placesHint: string;
  placeWhat: string;
  placePath: string;
  addPlace: string;
  removePlace: string;
  canon: string;
  canonHint: string;
  noCanon: string;
  open: string;
  save: string;
  saving: string;
  cancel: string;
  failed(why: string): string;
}

export const zhProjectForm: ProjectFormWords = {
  newTitle: "新项目",
  editTitle: (name) => `修改「${name}」`,
  name: "名字",
  namePlaceholder: "给它起个名字，比如 mark50 研发",
  slug: "短名",
  slugHint: "小写字母、数字与连字符，以字母开头；命令里、目录里用它，开了就不再改",
  what: "它是什么",
  whatPlaceholder: "一两句话，说这个项目为了什么",
  machines: "横跨哪几台机器",
  places: "在哪里能找到什么",
  placesHint: "一行一样东西：它是什么、在哪台机器、原生的绝对路径",
  placeWhat: "是什么",
  placePath: "路径",
  addPlace: "再加一行",
  removePlace: "删掉这一行",
  canon: "定论在哪",
  canonHint: "存放定论的文件夹",
  noCanon: "不写",
  open: "开这个项目",
  save: "存成新的一版",
  saving: "正在写…",
  cancel: "关闭",
  failed: (why) => `没能写下：${why}`,
};

export const enProjectForm: ProjectFormWords = {
  newTitle: "New project",
  editTitle: (name) => `Edit “${name}”`,
  name: "Name",
  namePlaceholder: "Name it, such as Mark 50 research",
  slug: "Short name",
  slugHint: "lowercase letters, digits and hyphens, starting with a letter; used in commands and directories, and never changed",
  what: "What it is",
  whatPlaceholder: "A sentence or two on what the project is for",
  machines: "Machines it spans",
  places: "Where things are",
  placesHint: "One thing a line: what it is, on which machine, its native absolute path",
  placeWhat: "What",
  placePath: "Path",
  addPlace: "Add a line",
  removePlace: "Remove this line",
  canon: "Decisions",
  canonHint: "the folder holding the decisions",
  noCanon: "none",
  open: "Open the project",
  save: "Save as the next version",
  saving: "Writing…",
  cancel: "Close",
  failed: (why) => `Could not write it: ${why}`,
};

export interface DeviceWords {
  invite: string;
  inviteTitle: string;
  inviteName: string;
  inviteNameHint: string;
  valid: string;
  hours(n: number): string;
  make: string;
  making: string;
  code: string;
  copy: string;
  copied: string;
  until(at: string): string;
  howTo: string;
  howToSteps: string[];
  again: string;
  revoke: string;
  revokeTitle(name: string): string;
  revokeWhat(name: string): string;
  reason: string;
  reasonDefault: string;
  revokeGo: string;
  leave: string;
  leaveTitle: string;
  leaveWhat: string;
  leaveGo: string;
  working: string;
  close: string;
  failed(why: string): string;
  specimen: string;
}

export const zhDevices: DeviceWords = {
  invite: "邀请一台设备",
  inviteTitle: "邀请一台设备",
  inviteName: "它叫什么",
  inviteNameHint: "小写字母、数字与连字符；不写就由它自己起名",
  valid: "多久内有效",
  hours: (n) => (n < 24 ? `${n} 小时` : `${n / 24} 天`),
  make: "生成邀请码",
  making: "正在生成…",
  code: "邀请码",
  copy: "复制",
  copied: "已复制",
  until: (at) => `${at}之前有效，只能用一次`,
  howTo: "在新设备上",
  howToSteps: [
    "装好 World，打开它，选「加入已有的世界」，扫这个码或粘贴邀请码；",
    "或者在它的终端里敲 fastctx join <邀请码>。",
    "加入时会请一次管理员确认，装上开机自启的服务，之后它就一直连着。",
  ],
  again: "再生成一个",
  revoke: "吊销",
  revokeTitle: (name) => `吊销 ${name}`,
  revokeWhat: (name) => `从此每台机器都不再认 ${name} 的钥匙：它不能再说话、收信，也不能再回来，除非重新邀请。它身上的东西不会被删掉。丢了的设备就这样处理。`,
  reason: "原因",
  reasonDefault: "revoked",
  revokeGo: "吊销它",
  leave: "离开世界",
  leaveTitle: "让这台离开世界",
  leaveWhat: "这台机器吊销自己的钥匙，回到独自一台。它身上的居民与记录都留着；别的机器从此不再认它。要回来，得重新被邀请。",
  leaveGo: "离开",
  working: "正在办…",
  close: "关闭",
  failed: (why) => `没办成：${why}`,
  specimen: "这是样例，没有连着世界",
};

export const enDevices: DeviceWords = {
  invite: "Invite a device",
  inviteTitle: "Invite a device",
  inviteName: "Its name",
  inviteNameHint: "lowercase letters, digits and hyphens; left empty, it names itself",
  valid: "Valid for",
  hours: (n) => (n < 24 ? `${n} hour${n === 1 ? "" : "s"}` : `${n / 24} day${n === 24 ? "" : "s"}`),
  make: "Make the code",
  making: "Making it…",
  code: "Invitation code",
  copy: "Copy",
  copied: "Copied",
  until: (at) => `valid until ${at}, once`,
  howTo: "On the new device",
  howToSteps: [
    "Install World, open it, choose “Join a World”, and scan this code or paste it;",
    "or run fastctx join <code> in its terminal.",
    "Joining asks for an administrator once to install the service that keeps it connected.",
  ],
  again: "Make another",
  revoke: "Revoke",
  revokeTitle: (name) => `Revoke ${name}`,
  revokeWhat: (name) => `No machine will trust ${name}'s key again: it can no longer speak, receive letters or come back unless invited anew. Nothing on it is deleted. This is what to do with a lost device.`,
  reason: "Reason",
  reasonDefault: "revoked",
  revokeGo: "Revoke it",
  leave: "Leave the World",
  leaveTitle: "Have this machine leave the World",
  leaveWhat: "This machine revokes its own key and becomes a World of one. Its residents and records stay; the other machines stop trusting it. To come back it must be invited again.",
  leaveGo: "Leave",
  working: "Working…",
  close: "Close",
  failed: (why) => `It did not go through: ${why}`,
  specimen: "This is a specimen, not connected to the World",
};

export interface PaletteWords {
  placeholder: string;
  nothing: string;
  actions: string;
  places: string;
  agents: string;
  machines: string;
  theme(name: string): string;
  fold: string;
  hint: string;
}

export const zhPalette: PaletteWords = {
  placeholder: "去哪里，做什么…",
  nothing: "没有对得上的",
  actions: "做",
  places: "地方",
  agents: "智能体",
  machines: "机器",
  theme: (name) => `主题：${name}`,
  fold: "收起或打开侧栏",
  hint: "↑↓ 选择 · Enter 前往 · Esc 关闭",
};

export const enPalette: PaletteWords = {
  placeholder: "Go somewhere, do something…",
  nothing: "Nothing matches",
  actions: "Do",
  places: "Places",
  agents: "Agents",
  machines: "Machines",
  theme: (name) => `Theme: ${name}`,
  fold: "Fold or open the sidebar",
  hint: "↑↓ to choose · Enter to go · Esc to close",
};

export interface WelcomeWords {
  lede: string;
  thisComputer: string;
  facts(name: string, world: string | null): string;
  placed: string;
  notPlaced: string;
  place: string;
  noCarried: string;
  serviceOn: string;
  serviceOff: string;
  join: string;
  joinWhat: string;
  found: string;
  foundWhat: string;
  alone: string;
  aloneWhat: string;
  code: string;
  codeHint: string;
  name: string;
  nameHint: string;
  joinGo: string;
  joining: string;
  hubWhere: string;
  hubHere: string;
  hubServer: string;
  address: string;
  addressHint: string;
  hubGo: string;
  founding: string;
  serverSteps: string[];
  foundingCode: string;
  joined(world: string, name: string): string;
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
  failed(why: string): string;
  back: string;
}

export const zhWelcome: WelcomeWords = {
  lede: "让你的几台电脑，和住在上面的 Codex、Claude，在同一个世界里。",
  thisComputer: "这台电脑",
  facts: (name, world) => (world ? `叫 ${name}，在世界 ${world} 里` : `叫 ${name}，此刻独自一台`),
  placed: "fastctx 已就位",
  notPlaced: "fastctx 还没放好",
  place: "放好它",
  noCarried: "这一份 World 没有随身带 fastctx",
  serviceOn: "开机自启的服务在",
  serviceOff: "还没有开机自启的服务",
  join: "加入已有的世界",
  joinWhat: "另一台设备在 World 里「邀请一台设备」，会给你一个邀请码。",
  found: "建一个新世界",
  foundWhat: "要有一个中枢，别的设备都连向它：可以是这台电脑，也可以是一台服务器。",
  alone: "先独自用",
  aloneWhat: "这台电脑本身就是一个世界。以后随时可以加入或建一个。",
  code: "邀请码",
  codeHint: "以 fxw3. 开头的一长串，粘贴进来",
  name: "这台电脑在世界里叫什么",
  nameHint: "小写字母、数字与连字符；不写就用它现在的名字",
  joinGo: "加入",
  joining: "正在加入，稍后会请一次管理员确认…",
  hubWhere: "中枢放在哪",
  hubHere: "这台电脑",
  hubServer: "一台服务器",
  address: "别的设备够得着这台电脑的地址",
  addressHint: "IP 加端口，比如 203.0.113.5:7443；它要一直开着，别的设备才连得上",
  hubGo: "在这台电脑上建中枢",
  founding: "正在建，稍后会请一次管理员确认…",
  serverSteps: [
    "在服务器上装好 fastctx（npm install -g fastctx，或从 GitHub 的发布页下载）；",
    "运行 fastctx hub init --public <服务器的公网 IP>:7443，它会打印一个建世界码；",
    "运行 sudo fastctx service hub install，让中枢开机自启、一直在；",
    "把那个建世界码粘贴到上面「加入已有的世界」里，这台电脑就是这个世界的第一台。",
  ],
  foundingCode: "建世界码",
  joined: (world, name) => `已经在世界 ${world} 里，这台叫 ${name}。`,
  kept: {
    already: "开机自启的服务早就在了。",
    installed: "开机自启的服务装好了：从此开机就连着，不管有没有人登录。",
    declined: "没有拿到管理员确认，所以没装开机服务：只有你登录着时它才连着。以后可以在设置里再装。",
    unsupported: "这个系统没有开机服务：只有你登录着时它才连着。",
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
  failed: (why) => `没办成：${why}`,
  back: "返回",
};

export const enWelcome: WelcomeWords = {
  lede: "Your computers, and the Codex and Claude living on them, in one World.",
  thisComputer: "This computer",
  facts: (name, world) => (world ? `named ${name}, in the World ${world}` : `named ${name}, on its own for now`),
  placed: "fastctx is in place",
  notPlaced: "fastctx is not in place yet",
  place: "Put it in place",
  noCarried: "this build of World carries no fastctx",
  serviceOn: "a service keeps it running",
  serviceOff: "no service keeps it running yet",
  join: "Join a World",
  joinWhat: "Another device's World gives you an invitation code under “Invite a device”.",
  found: "Found a new World",
  foundWhat: "It needs a hub every device connects to: this computer, or a server.",
  alone: "Stay on its own",
  aloneWhat: "This computer is a World by itself. You can join or found one any time.",
  code: "Invitation code",
  codeHint: "a long string starting with fxw3.; paste it here",
  name: "This computer's name in the World",
  nameHint: "lowercase letters, digits and hyphens; left empty, it keeps its name",
  joinGo: "Join",
  joining: "Joining; an administrator will be asked once…",
  hubWhere: "Where the hub is",
  hubHere: "This computer",
  hubServer: "A server",
  address: "An address other devices reach this computer at",
  addressHint: "IP and port, such as 203.0.113.5:7443; it must stay on for the others to connect",
  hubGo: "Found the hub here",
  founding: "Founding; an administrator will be asked once…",
  serverSteps: [
    "Install fastctx on the server (npm install -g fastctx, or from the GitHub releases);",
    "run fastctx hub init --public <the server's public IP>:7443; it prints a founding code;",
    "run sudo fastctx service hub install, so the hub starts at boot and stays up;",
    "paste the founding code into “Join a World” above; this computer is the World's first.",
  ],
  foundingCode: "Founding code",
  joined: (world, name) => `In the World ${world}, as ${name}.`,
  kept: {
    already: "A service already keeps it running.",
    installed: "A service now keeps it running: connected from boot, signed in or not.",
    declined: "Administrator rights were declined, so no service was installed: it is connected only while you are signed in. You can install it later in the settings.",
    unsupported: "This system has no service for it: it is connected only while you are signed in.",
  },
  keptCommand: "To keep it connected from boot, run in a terminal:",
  hosts: "Put the World's tools into the agents on this computer",
  hostsWhat: "Then the Codex and Claude Code you open on this computer yourself use the World's hands and see the World. You can take them out any time.",
  codex: "Codex",
  claude: "Claude Code",
  hostState: { absent: "not installed", out: "not in", in: "in", old: "an old fastctx is in it" },
  applyGo: "Put them in",
  applying: "Putting them in…",
  enter: "Enter the World",
  failed: (why) => `It did not go through: ${why}`,
  back: "Back",
};

export interface ComposerWords {
  asks: AskWords;
  commands: {
    world: string;
    compact: string;
    model: string;
    rename: string;
    fork: string;
    stop: string;
    noCommands: string;
    effort: string;
    current: string;
  };
  modelChip: string;
  noModels: string;
  unknown(name: string): string;
  running(name: string): string;
  done(name: string): string;
  failed(why: string): string;
  context(percent: number, used: number, window: number): string;
}

export const zhComposer: ComposerWords = {
  asks: {
    waiting: "在等你回答",
    command: (machine) => `想在 ${machine} 上跑一条命令`,
    stdin: (machine) => `想往 ${machine} 上一个在跑的程序里送输入`,
    network: (host, protocol) => `想连上 ${host}${protocol ? `（${protocol}）` : ""}`,
    files: "想改文件",
    filesNamed: (count) => `想改 ${count} 个文件`,
    grantRoot: (root) => `并请你允许它在这段对话里随意写 ${root} 下的东西`,
    reason: (why) => `它说：${why}`,
    inDir: (cwd) => `在 ${cwd}`,
    accept: "允许",
    acceptForSession: "这段对话里都允许",
    acceptPrefix: (prefix) => `以后 ${prefix} 开头的都允许`,
    networkRule: (allow, host) => (allow ? `以后都允许连 ${host}` : `以后都不许连 ${host}`),
    decline: "不允许，让它接着做",
    cancel: "不允许，停下这一轮",
    question: "它有一个问题",
    questions: (count) => `它有 ${count} 个问题`,
    otherAnswer: "或者写下你自己的回答",
    secret: "写下的字不会显示出来",
    submit: "回答",
    fill: "提交",
    skip: "跳过",
    form: (server) => `${server} 想请你填一张表`,
    link: (server) => `${server} 请你打开一个网址`,
    openLink: "打开网址，接着做",
    refuse: "拒绝",
    dismiss: "先不管",
    required: "这一项要填",
    formUnreadable: "这张表在这里画不出来，只能拒绝或先不管",
    sending: "正在送出…",
    failed: (why) => `没送出去：${why}`,
    cannot: (request) => `它在等一个这里还答不了的回答（${request}）`,
    permissions: "它在请你给它更多权限，这里还答不了",
    verification: "它在请你做一次身份验证，这里还答不了",
    answered: (said) => said,
    gone: "已在别处答过，或不再需要了",
    settledBy: {
      accept: "你允许了",
      session: "你允许了，这段对话里不再问",
      rule: "你定下了一条以后照办的规矩",
      decline: "你没有允许，它接着做别的",
      cancel: "你没有允许，这一轮停下了",
      answered: "你回答了",
      skipped: "你跳过了",
      submitted: "你提交了",
      refused: "你拒绝了",
      dismissed: "你没有理会",
      opened: "你打开了网址",
    },
  },
  commands: {
    world: "世界",
    compact: "现在压缩这段对话",
    model: "换模型与推理强度",
    rename: "给这段对话起名",
    fork: "从这里分叉出一段新的对话",
    stop: "停下这一轮",
    noCommands: "没有对得上的命令",
    effort: "推理强度",
    current: "现在用的",
  },
  modelChip: "模型",
  noModels: "它没有报出可选的模型",
  unknown: (name) => `没有 /${name} 这个命令`,
  running: (name) => `正在执行 /${name}…`,
  done: (name) => `/${name} 做完了`,
  failed: (why) => `没做成：${why}`,
  context: (percent, used, window) => `上下文用了 ${percent}%（${used.toLocaleString()} / ${window.toLocaleString()}）`,
};

export const enComposer: ComposerWords = {
  asks: {
    waiting: "Waiting for you",
    command: (machine) => `Wants to run a command on ${machine}`,
    stdin: (machine) => `Wants to send input to a program running on ${machine}`,
    network: (host, protocol) => `Wants to reach ${host}${protocol ? ` (${protocol})` : ""}`,
    files: "Wants to change files",
    filesNamed: (count) => `Wants to change ${count} ${count === 1 ? "file" : "files"}`,
    grantRoot: (root) => `and asks to write anything under ${root} for the rest of this conversation`,
    reason: (why) => `It says: ${why}`,
    inDir: (cwd) => `In ${cwd}`,
    accept: "Allow",
    acceptForSession: "Allow for this conversation",
    acceptPrefix: (prefix) => `Always allow commands starting ${prefix}`,
    networkRule: (allow, host) => (allow ? `Always allow ${host}` : `Never allow ${host}`),
    decline: "Don't allow, carry on",
    cancel: "Don't allow, stop this turn",
    question: "It has a question",
    questions: (count) => `It has ${count} questions`,
    otherAnswer: "Or write your own answer",
    secret: "What you type is hidden",
    submit: "Answer",
    fill: "Submit",
    skip: "Skip",
    form: (server) => `${server} asks you to fill in a form`,
    link: (server) => `${server} asks you to open a link`,
    openLink: "Open it and carry on",
    refuse: "Refuse",
    dismiss: "Not now",
    required: "This one is needed",
    formUnreadable: "This form can't be drawn here; it can only be refused or left",
    sending: "Sending…",
    failed: (why) => `Not sent: ${why}`,
    cannot: (request) => `It waits for an answer that can't be given here (${request})`,
    permissions: "It asks for more permissions, which can't be granted here",
    verification: "It asks you to verify who you are, which can't be done here",
    answered: (said) => said,
    gone: "Answered elsewhere, or no longer needed",
    settledBy: {
      accept: "You allowed it",
      session: "You allowed it for this conversation",
      rule: "You set a rule it keeps from now on",
      decline: "You didn't allow it; it carried on",
      cancel: "You didn't allow it; the turn stopped",
      answered: "You answered",
      skipped: "You skipped it",
      submitted: "You submitted it",
      refused: "You refused",
      dismissed: "You left it",
      opened: "You opened the link",
    },
  },
  commands: {
    world: "World",
    compact: "Compact this conversation now",
    model: "Change the model and effort",
    rename: "Name this conversation",
    fork: "Fork a new conversation from here",
    stop: "Stop this turn",
    noCommands: "No command matches",
    effort: "effort",
    current: "in use",
  },
  modelChip: "Model",
  noModels: "It lists no models to choose from",
  unknown: (name) => `There is no /${name}`,
  running: (name) => `Running /${name}…`,
  done: (name) => `/${name} done`,
  failed: (why) => `It did not go through: ${why}`,
  context: (percent, used, window) => `${percent}% of the context used (${used.toLocaleString()} of ${window.toLocaleString()})`,
};

function zhSpan(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "不到一分钟";
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时`;
  return `${Math.floor(hours / 24)} 天`;
}

function enSpan(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h`;
  const days = Math.floor(hours / 24);
  return `${days} day${days === 1 ? "" : "s"}`;
}

export const zhShell: ShellWords = {
  app: "World",
  world: "世界",
  projects: "项目",
  outside: "不在项目里",
  machines: "机器",
  settings: "设置",
  newTalk: "新对话",
  newProject: "新项目",
  edit: "修改",
  openSidebar: "打开侧栏",
  foldSidebar: "收起侧栏",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueFor: (asker) => `受 ${asker} 之托`,
  busy: (lasted) => `在干活 · ${lasted}`,
  idle: "空闲",
  waiting: "等你回答",
  asleep: "睡着了",
  offline: "离线",
  noProjects: "还没有项目",
  nobodyHere: "此刻没有谁在",
  unreachable: "问不到世界",
  thisOne: "这台",
  joinOrFound: "加入或建一个世界",
  more: (n) => `还有 ${n} 个`,
  span: zhSpan,
};

export const enShell: ShellWords = {
  app: "World",
  world: "The World",
  projects: "Projects",
  outside: "In no project",
  machines: "Machines",
  settings: "Settings",
  newTalk: "New conversation",
  newProject: "New project",
  edit: "Edit",
  openSidebar: "Open the sidebar",
  foldSidebar: "Fold the sidebar",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueFor: (asker) => `for ${asker}`,
  busy: (lasted) => `working · ${lasted}`,
  idle: "idle",
  waiting: "waiting for you",
  asleep: "asleep",
  offline: "offline",
  noProjects: "No projects yet",
  nobodyHere: "Nobody here right now",
  unreachable: "The World cannot be asked",
  thisOne: "this one",
  joinOrFound: "Join or found a World",
  more: (n) => `${n} more`,
  span: enSpan,
};
