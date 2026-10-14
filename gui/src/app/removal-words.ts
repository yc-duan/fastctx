// What the settings say about taking fastctx off this computer
// (`world-gui.md`, 设置): the two ways, what each takes away and leaves, and
// how it went. Short; the longer sense of each way is a hover away.

import { enProblems, failure, zhProblems } from "./problem";

export interface RemovalWords {
  title: string;
  /** The three steps: choosing a way, seeing what it does, doing it. */
  stages: [string, string, string];
  back: string;
  /** How many of the steps are done, of all. */
  count(done: number, all: number): string;
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
    case "claude-work-cache":
    case "claude-shared-cache":
    case "claude-trust":
      return "agents";
    case "data":
    case "legacy":
    case "user-files":
    case "unrecognized":
    case "unremoved":
    case "removal-records":
    case "settings-copies":
      return "data";
    case "logout":
      return "hubs";
    default:
      return "program";
  }
}

export const zhRemoval: RemovalWords = {
  title: "卸载",
  stages: ["选择", "确认", "移除"],
  back: "返回",
  count: (done, all) => `${done} / ${all}`,
  open: "卸载 Fastctx",
  keep: "保留数据",
  keepWhat: "重新安装后继续使用",
  keepMore: "移除程序、开机服务与 Codex、Claude Code 中的接入；保留记录、登录和机器身份。",
  erase: "彻底移除",
  eraseWhat: "清除 fastctx 在这台电脑上的一切",
  eraseMore: "清除 fastctx 写入的所有内容，不影响你自己的文件与设置。移除前会先退出已登录的中枢服务器。",
  goes: "移除",
  stays: "保留",
  groups: {
    start: "开机自启",
    hub: "中枢服务器",
    agents: "智能体接入",
    program: "程序",
    data: "数据",
    hubs: "中枢服务器上的本机",
  },
  kinds: {
    service: "开机服务",
    task: "计划任务",
    "hub-service": "中枢服务器的系统服务",
    firewall: "防火墙规则",
    "hub-data": "中枢服务器数据",
    codex: "Codex 接入",
    claude: "Claude Code 接入",
    "claude-cache": "Claude Code 插件缓存",
    "claude-work-cache": "Claude Code 缓存、日志与临时文件",
    "claude-shared-cache": "归属不明的 Claude Code 缓存",
    "claude-trust": "Claude Code 目录信任",
    program: "fastctx 程序",
    path: "PATH 条目",
    app: "Fastctx 应用",
    data: "记录与登录",
    legacy: "旧版归档",
    logout: "退出中枢服务器",
    runtime: "运行中的进程",
    "other-program": "其他 fastctx",
    "user-files": "文件的原始版本",
    unrecognized: "不是 Fastctx 的文件",
    unremoved: "没删掉的文件",
    "removal-records": "卸载自己的记录",
    "settings-copies": "宿主设置的副本",
    "claude-permission": "Claude 自动运行授权",
    "account-right": "「作为批处理作业登录」权限",
    linger: "systemd linger",
  },
  leftWhy: {
    data: "按你的选择保留",
    "claude-work-cache": "按你的选择保留",
    "claude-shared-cache": "归属不能确定，原样保留",
    legacy: "按你的选择保留",
    "other-program": "不归 Fastctx 管理，保持不变",
    "user-files": "保留你文件的原始版本，避免丢失之后的写入",
    unrecognized: "不是 Fastctx 写的，原样留着",
    "removal-records": "Fastctx 退出后随安装包删除",
    "settings-copies": "没能删掉，里面可能有登录令牌，请手动删除",
    "claude-permission": "这是你给的授权，原值未记录，保持不变",
    "account-right": "安装前的状态未记录，保持不变",
    linger: "安装前的状态未记录，保持不变",
    program: "由 npm 或其他安装方式管理",
    app: "Fastctx 退出后由安装包移除",
  },
  unfinished: "未完成",
  failedRun: failure(zhProblems, "卸载未完成"),
  hubWarn: (world) => `这台电脑是「${world}」的中枢服务器。彻底移除将删除这个网络的所有账号、机器和项目，其他电脑将无法再连接。`,
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
  finish: "关闭并卸载 Fastctx",
  finishWhat: "Fastctx 将关闭并从这台电脑上卸载",
  finishing: "正在关闭…",
};

export const enRemoval: RemovalWords = {
  title: "Uninstall",
  stages: ["Choose", "Check", "Remove"],
  back: "Back",
  count: (done, all) => `${done} / ${all}`,
  open: "Uninstall Fastctx",
  keep: "Keep data",
  keepWhat: "Pick up where you left off after reinstalling",
  keepMore: "Removes the program, startup service and the Codex and Claude Code integrations; keeps records, logins and this machine's identity.",
  erase: "Remove everything",
  eraseWhat: "Erase everything fastctx put on this computer",
  eraseMore: "Erases everything fastctx wrote, leaving your own files and settings alone. Signs out of its hubs first.",
  goes: "Removed",
  stays: "Kept",
  groups: {
    start: "Startup",
    hub: "Hub",
    agents: "Agent integrations",
    program: "Program",
    data: "Data",
    hubs: "This computer on hubs",
  },
  kinds: {
    service: "Startup service",
    task: "Scheduled task",
    "hub-service": "Hub service",
    firewall: "Firewall rule",
    "hub-data": "Hub data",
    codex: "Codex integration",
    claude: "Claude Code integration",
    "claude-cache": "Claude Code plugin cache",
    "claude-work-cache": "Claude Code cache, logs and temporary files",
    "claude-shared-cache": "Claude Code cache with unproven ownership",
    "claude-trust": "Claude Code folder trust",
    program: "fastctx program",
    path: "PATH entry",
    app: "Fastctx app",
    data: "Records and logins",
    legacy: "Old version archives",
    logout: "Sign out of hub",
    runtime: "Running processes",
    "other-program": "Other fastctx",
    "user-files": "Original file versions",
    unrecognized: "Files that aren't Fastctx's",
    unremoved: "Files not removed",
    "removal-records": "The removal's own records",
    "settings-copies": "Copies of host settings",
    "claude-permission": "Claude unattended permission",
    "account-right": "“Log on as a batch job” right",
    linger: "systemd linger",
  },
  leftWhy: {
    data: "Kept as you chose",
    "claude-work-cache": "Kept as you chose",
    "claude-shared-cache": "Ownership is unproven; kept unchanged",
    legacy: "Kept as you chose",
    "other-program": "Not managed by Fastctx, so it stays",
    "user-files": "Kept so later writes to your files aren't lost",
    unrecognized: "Not written by Fastctx, so it stays",
    "removal-records": "Removed with the package once Fastctx exits",
    "settings-copies": "Couldn't be removed and may hold a login token; delete it by hand",
    "claude-permission": "Your own permission; its earlier value wasn't recorded, so it stays",
    "account-right": "Its state before installation wasn't recorded, so it stays",
    linger: "Its state before installation wasn't recorded, so it stays",
    program: "Managed by npm or another installer",
    app: "Removed by the package after Fastctx exits",
  },
  unfinished: "Not finished",
  failedRun: failure(enProblems, "Uninstall didn't finish"),
  hubWarn: (world) => `This computer is the hub for ${world}. Removing everything deletes all its accounts, machines and projects, and no other computer will be able to connect.`,
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
  finish: "Close and uninstall Fastctx",
  finishWhat: "Fastctx will close and uninstall itself",
  finishing: "Closing…",
};
