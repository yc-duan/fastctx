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
  leave: string;
  leaveTitle(hub: string): string;
  leaveWhat(hub: string): string;
  leaveGo: string;
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
  whose: (owner, machine, hub) => `${owner} 的 ${machine} · 在 ${hub}`,
  online: "已连接",
  onlineMs: (ms) => `已连接 · ${ms} 毫秒`,
  away: (at) => `断开 · 从 ${at} 起`,
  removed: "已被移出网络",
  removedWhose: (owner, hub) => `原是 ${owner} 在 ${hub} 里的机器`,
  rejoin: "用入网码重新加入",
  leave: "离开网络",
  leaveTitle: (hub) => `离开 ${hub}？`,
  leaveWhat: (hub) => `这台电脑不再是 ${hub} 里的受管机器，变回一台全新的电脑。要回来，用入网码重新加入。`,
  leaveGo: "离开",
  uninstall: "卸载",
  unkept: "开机服务没装上：没人登录这个系统账户时，这台就断开",
  keep: "装上",
  keeping: "正在装…",
  keepCommand: "在这个账户的终端里运行：",
};

export const enManaged: ManagedWords = {
  title: "Managed machine",
  whose: (owner, machine, hub) => `${owner}'s ${machine} · on ${hub}`,
  online: "Connected",
  onlineMs: (ms) => `Connected · ${ms} ms`,
  away: (at) => `Disconnected since ${at}`,
  removed: "Taken off the network",
  removedWhose: (owner, hub) => `It was ${owner}'s machine on ${hub}`,
  rejoin: "Join again with a code",
  leave: "Leave the network",
  leaveTitle: (hub) => `Leave ${hub}?`,
  leaveWhat: (hub) => `This computer stops being a managed machine on ${hub} and starts fresh. To come back, join again with a code.`,
  leaveGo: "Leave",
  uninstall: "Uninstall",
  unkept: "The startup service isn't installed: while no one is signed in to this system account, this computer is disconnected",
  keep: "Install",
  keeping: "Installing…",
  keepCommand: "Run in this account's terminal:",
};
