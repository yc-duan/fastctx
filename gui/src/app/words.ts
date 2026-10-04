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
