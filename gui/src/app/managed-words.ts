// What a managed machine's one page says (`world-gui.md`, 受管机器上的界面),
// in the user's words (`world-gui.md`, 用词).

export interface ManagedWords {
  title: string;
  /** Whose machine it is, and on which network. */
  whose(owner: string, machine: string, hub: string): string;
  online: string;
  onlineMs(ms: number): string;
  /** Not connected, since `at`. */
  away(at: string): string;
  /** Its owner took it off the network. */
  removed: string;
  removedWhose(owner: string, hub: string): string;
  rejoin: string;
  uninstall: string;
  /** Without the startup service it is connected only while its system
   *  account is signed in: said, with the way to install it. */
  unkept: string;
  keep: string;
  keeping: string;
  keepCommand: string;
}

export const zhManaged: ManagedWords = {
  title: "受管机器",
  whose: (owner, machine, hub) => `${hub} 上 ${owner} 的 ${machine}`,
  online: "已连接",
  onlineMs: (ms) => `已连接　${ms} 毫秒`,
  away: (at) => `${at} 起断开`,
  removed: "已被移出网络",
  removedWhose: (owner, hub) => `原为 ${owner} 在 ${hub} 中的机器`,
  rejoin: "用入网码重新加入",
  uninstall: "卸载",
  unkept: "开机服务未安装：无人登录此系统账户时，本机将断开",
  keep: "安装",
  keeping: "正在安装…",
  keepCommand: "在此账户的终端中运行：",
};

export const enManaged: ManagedWords = {
  title: "Managed machine",
  whose: (owner, machine, hub) => `${owner}'s ${machine} on ${hub}`,
  online: "Connected",
  onlineMs: (ms) => `Connected, ${ms} ms`,
  away: (at) => `Disconnected since ${at}`,
  removed: "Taken off the network",
  removedWhose: (owner, hub) => `It was ${owner}'s machine on ${hub}`,
  rejoin: "Join again with a code",
  uninstall: "Uninstall",
  unkept: "The startup service isn't installed: while no one is signed in to this system account, this computer is disconnected",
  keep: "Install",
  keeping: "Installing…",
  keepCommand: "Run in this account's terminal:",
};
