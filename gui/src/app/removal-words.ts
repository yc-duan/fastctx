// What the settings say about taking fastctx off this computer
// (`world-gui.md`, 设置): the two ways, what each takes away and leaves, and
// how it went. Short; the longer sense of each way is a hover away.

import { enProblems, failure, zhProblems } from "./problem";

export interface RemovalWords {
  title: string;
  /** The settings' row that opens the uninstall dialog. */
  open: string;
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
  /** Why an item of a kind was left on purpose; the kernel's reason is the
   *  hover. */
  leftWhy: Record<string, string>;
  unfinished: string;
  failedRun(why: string): string;
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
    case "claude-cache":
    case "claude-trust":
      return "agents";
    case "data":
    case "legacy":
    case "user-files":
      return "data";
    case "logout":
      return "hubs";
    default:
      return "program";
  }
}

export const zhRemoval: RemovalWords = {
  title: "卸载",
  open: "卸载 World",
  keep: "保留数据",
  keepWhat: "重新安装后继续使用",
  keepMore: "移除程序、开机服务与 Codex、Claude Code 中的接入；保留记录、登录和机器身份。",
  erase: "彻底移除",
  eraseWhat: "清除 fastctx 在这台电脑上的一切",
  eraseMore: "清除 fastctx 写入的所有内容，不影响你自己的文件与设置。移除前会先退出已登录的服务器。",
  goes: "移除",
  stays: "保留",
  groups: {
    start: "开机自启",
    hub: "服务器",
    agents: "智能体接入",
    program: "程序",
    data: "数据",
    hubs: "服务器上的本机",
  },
  kinds: {
    service: "开机服务",
    task: "计划任务",
    "hub-service": "服务器服务",
    firewall: "防火墙规则",
    "hub-data": "服务器数据",
    codex: "Codex 接入",
    claude: "Claude Code 接入",
    "claude-cache": "Claude Code 插件缓存",
    "claude-trust": "Claude Code 目录信任",
    program: "fastctx 程序",
    path: "PATH 条目",
    app: "World 应用",
    data: "记录与登录",
    legacy: "旧版归档",
    logout: "退出服务器",
    runtime: "运行中的进程",
    "other-program": "其他 fastctx",
    "user-files": "文件的原始版本",
    "claude-permission": "Claude 自动运行授权",
    "account-right": "「作为批处理作业登录」权限",
    linger: "systemd linger",
  },
  leftWhy: {
    data: "按你的选择保留",
    legacy: "按你的选择保留",
    "other-program": "不归 World 管理，保持不变",
    "user-files": "保留你文件的原始版本，避免丢失之后的写入",
    "claude-permission": "这是你给的授权，原值未记录，保持不变",
    "account-right": "安装前的状态未记录，保持不变",
    linger: "安装前的状态未记录，保持不变",
    program: "由 npm 或其他安装方式管理",
    app: "World 退出后由安装包移除",
  },
  unfinished: "未完成",
  failedRun: failure(zhProblems, "卸载未完成"),
  hubWarn: (world) => `这台电脑是「${world}」的服务器。彻底移除将删除这个世界的所有账号、机器和项目，其他电脑将无法再连接。`,
  typeName: (world) => `输入「${world}」以确认`,
  other: (path, version) => `PATH 中另有一个 fastctx${version ? ` ${version}` : ""}，不会被移除：${path}`,
  list: "查看清单",
  hideList: "收起清单",
  admin: "需要管理员权限",
  go: "卸载",
  goErase: "彻底移除",
  again: "再次点击确认",
  cancel: "取消",
  planning: "正在检查…",
  running: "正在移除…",
  failed: failure(zhProblems, "无法获取移除清单"),
  left: "未能移除",
  allDone: "已全部移除",
  finish: "关闭并卸载 World",
  finishWhat: "World 将关闭并从这台电脑上卸载",
  finishing: "正在关闭…",
};

export const enRemoval: RemovalWords = {
  title: "Uninstall",
  open: "Uninstall World",
  keep: "Keep data",
  keepWhat: "Pick up where you left off after reinstalling",
  keepMore: "Removes the program, startup service and the Codex and Claude Code integrations; keeps records, logins and this machine's identity.",
  erase: "Remove everything",
  eraseWhat: "Erase everything fastctx put on this computer",
  eraseMore: "Erases everything fastctx wrote, leaving your own files and settings alone. Signs out of its servers first.",
  goes: "Removed",
  stays: "Kept",
  groups: {
    start: "Startup",
    hub: "Server",
    agents: "Agent integrations",
    program: "Program",
    data: "Data",
    hubs: "This computer on servers",
  },
  kinds: {
    service: "Startup service",
    task: "Scheduled task",
    "hub-service": "Server service",
    firewall: "Firewall rule",
    "hub-data": "Server data",
    codex: "Codex integration",
    claude: "Claude Code integration",
    "claude-cache": "Claude Code plugin cache",
    "claude-trust": "Claude Code folder trust",
    program: "fastctx program",
    path: "PATH entry",
    app: "World app",
    data: "Records and logins",
    legacy: "Old version archives",
    logout: "Sign out of server",
    runtime: "Running processes",
    "other-program": "Other fastctx",
    "user-files": "Original file versions",
    "claude-permission": "Claude unattended permission",
    "account-right": "“Log on as a batch job” right",
    linger: "systemd linger",
  },
  leftWhy: {
    data: "Kept as you chose",
    legacy: "Kept as you chose",
    "other-program": "Not managed by World, so it stays",
    "user-files": "Kept so later writes to your files aren't lost",
    "claude-permission": "Your own permission; its earlier value wasn't recorded, so it stays",
    "account-right": "Its state before installation wasn't recorded, so it stays",
    linger: "Its state before installation wasn't recorded, so it stays",
    program: "Managed by npm or another installer",
    app: "Removed by the package after World exits",
  },
  unfinished: "Not finished",
  failedRun: failure(enProblems, "Uninstall didn't finish"),
  hubWarn: (world) => `This computer is the server for ${world}. Removing everything deletes all its accounts, machines and projects, and no other computer will be able to connect.`,
  typeName: (world) => `Type ${world} to confirm`,
  other: (path, version) => `Another fastctx${version ? ` ${version}` : ""} on PATH will stay: ${path}`,
  list: "Show list",
  hideList: "Hide list",
  admin: "Needs administrator rights",
  go: "Uninstall",
  goErase: "Remove everything",
  again: "Click again to confirm",
  cancel: "Cancel",
  planning: "Checking…",
  running: "Removing…",
  failed: failure(enProblems, "Couldn't list what to remove"),
  left: "Couldn't remove",
  allDone: "Everything removed",
  finish: "Close and uninstall World",
  finishWhat: "World will close and uninstall itself",
  finishing: "Closing…",
};
