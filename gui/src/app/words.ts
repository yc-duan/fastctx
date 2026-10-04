// What the frame around every page says: the sidebar, the new conversation,
// the settings. In the user's words (`world-gui.md`, 用词).

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
  more: (n) => `${n} more`,
  span: enSpan,
};
