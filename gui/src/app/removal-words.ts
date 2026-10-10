// What the settings say about taking fastctx off this computer
// (`world-gui.md`, 设置): the two ways, what each takes away and leaves, and
// how it went. Short; the longer sense of each way is a hover away.

export interface RemovalWords {
  title: string;
  keep: string;
  keepWhat: string;
  keepMore: string;
  erase: string;
  eraseWhat: string;
  eraseMore: string;
  goes: string;
  stays: string;
  /** A group of things on this computer, by what they are. */
  groups: Record<RemovalGroup, string>;
  /** Taking one item away, in a few words, by its kind. */
  kinds: Record<string, string>;
  hubWarn(world: string): string;
  typeName(world: string): string;
  other(path: string, version: string | null): string;
  list: string;
  hideList: string;
  admin: string;
  go: string;
  goErase: string;
  again: string;
  cancel: string;
  planning: string;
  running: string;
  failed(why: string): string;
  left: string;
  allDone: string;
  finish: string;
  finishWhat: string;
  finishing: string;
}

export type RemovalGroup = "start" | "hub" | "agents" | "program" | "data" | "hubs";

/** Which group each kind of item belongs to; an unknown kind goes with the
 *  program. */
export function groupOf(kind: string): RemovalGroup {
  switch (kind) {
    case "service":
    case "task":
      return "start";
    case "hub-service":
    case "firewall":
    case "hub-data":
      return "hub";
    case "codex":
    case "claude":
    case "claude-trust":
      return "agents";
    case "data":
    case "legacy":
      return "data";
    case "logout":
      return "hubs";
    default:
      return "program";
  }
}

export const zhRemoval: RemovalWords = {
  title: "移除",
  keep: "保留数据",
  keepWhat: "拿走程序，留下世界的记录",
  keepMore:
    "开机服务、计划任务、Codex 与 Claude Code 里的接入、PATH 里的那一项和程序都拿走；世界的记录、登录和这台机器的身份留着，重新装上就接着用。",
  erase: "彻底移除",
  eraseWhat: "这台电脑上不留痕迹",
  eraseMore:
    "fastctx 写在这台电脑上的一切都清掉，你自己的文件与设置一个字节都不碰。删之前先告诉登录着的中枢：这台电脑走了。",
  goes: "拿走",
  stays: "留下",
  groups: {
    start: "开机自启",
    hub: "中枢",
    agents: "智能体的接入",
    program: "程序",
    data: "世界的数据",
    hubs: "中枢上的这台电脑",
  },
  kinds: {
    service: "开机服务",
    task: "计划任务",
    "hub-service": "中枢的服务",
    firewall: "防火墙放行",
    "hub-data": "中枢的数据",
    codex: "Codex 接入",
    claude: "Claude Code 接入",
    "claude-trust": "Claude Code 里的目录信任",
    program: "fastctx 程序",
    path: "PATH 里的一项",
    app: "World 本身",
    data: "世界的记录与登录",
    legacy: "旧版的归档",
    logout: "从中枢退出",
  },
  hubWarn: (world) => `这台电脑是「${world}」的中枢。彻底移除会把这个世界的账号、机器和项目记录一起删掉，别的电脑再也连不上它。`,
  typeName: (world) => `输入「${world}」确认`,
  other: (path, version) => `PATH 上还有一份不归世界管的 fastctx${version ? ` ${version}` : ""}，它不会被拿走：${path}`,
  list: "看清单",
  hideList: "收起清单",
  admin: "要确认一次管理员权限",
  go: "移除",
  goErase: "彻底移除",
  again: "再按一次",
  cancel: "算了",
  planning: "正在看这台电脑上有什么……",
  running: "正在移除……",
  failed: (why) => `没能列出要移除的东西：${why}`,
  left: "没能拿走的",
  allDone: "都拿走了。",
  finish: "关上并移除 World",
  finishWhat: "World 会关上，再把自己从这台电脑上拿掉。",
  finishing: "正在关上……",
};

export const enRemoval: RemovalWords = {
  title: "Remove",
  keep: "Keep data",
  keepWhat: "Take the program, keep the World's records",
  keepMore:
    "The start-up service, scheduled task, the World in Codex and Claude Code, the PATH entry and the program go; the World's records, logins and this machine's identity stay, so installing again picks up where it left off.",
  erase: "Remove everything",
  eraseWhat: "Leave no trace on this computer",
  eraseMore:
    "Everything fastctx wrote on this computer goes; your own files and settings are not touched. The hubs it is logged in to are told first that it is leaving.",
  goes: "Goes",
  stays: "Stays",
  groups: {
    start: "Starting with the computer",
    hub: "Hub",
    agents: "In the agents",
    program: "Program",
    data: "The World's data",
    hubs: "This computer on its hubs",
  },
  kinds: {
    service: "Start-up service",
    task: "Scheduled task",
    "hub-service": "Hub service",
    firewall: "Firewall rule",
    "hub-data": "Hub data",
    codex: "In Codex",
    claude: "In Claude Code",
    "claude-trust": "Folder trust in Claude Code",
    program: "fastctx program",
    path: "PATH entry",
    app: "World itself",
    data: "Records and logins",
    legacy: "Old version's archives",
    logout: "Log out of hub",
  },
  hubWarn: (world) => `This computer is the hub of ${world}. Removing everything deletes that World's accounts, machines and project records with it; no other computer can reach it again.`,
  typeName: (world) => `Type ${world} to confirm`,
  other: (path, version) => `Another fastctx${version ? ` ${version}` : ""} on the PATH is not the World's and stays: ${path}`,
  list: "Show the list",
  hideList: "Hide the list",
  admin: "Asks for administrator rights once",
  go: "Remove",
  goErase: "Remove everything",
  again: "Press again",
  cancel: "Cancel",
  planning: "Looking at what is on this computer…",
  running: "Removing…",
  failed: (why) => `Could not list what to remove: ${why}`,
  left: "Not removed",
  allDone: "All of it is gone.",
  finish: "Close and remove World",
  finishWhat: "World closes, then takes itself off this computer.",
  finishing: "Closing…",
};
