// The words of a machine's 「管理」 (`machine-fastctx.ts`, `machine-remove.ts`):
// the Fastctx on it, how an update of it goes, and taking it off the network
// three ways.

import type { RenewalPhase } from "../world/renewal";
import type { CopiesWords } from "./machine-copies";
import { enProblems, failure, zhProblems } from "./problem";

export interface ManageWords {
  close: string;
  fastctx: string;
  /** The application that carries its World program: of the same version,
   *  or of its own. */
  withApp: string;
  app(version: string): string;
  upToDate: string;
  /** The newest it can take, on its own line. */
  canTake(version: string): string;
  update: string;
  /** A package on its way there: what came of how much, and from where. */
  fetching(received: string, total: string | null, network: boolean): string;
  phase: Record<RenewalPhase, string>;
  /** An update that did not take: a known reason in plain words, else
   *  that it failed; the kernel's words go in the hover. */
  failed(why: string): string;
  /** Its update could not be asked about. */
  unread(why: string): string;
  /** An update asked of a computer an application carries Fastctx on. */
  appTakes(version: string): string;
  appClosed(version: string): string;
  appOpenThere(version: string): string;
  /** A computer of an earlier release with the app: updated at it. */
  olderApp(version: string): string;
  /** An update the hub keeps for it until it is back. */
  due(version: string, by: string): string;
  away: string;
  /** Why a machine of an earlier release has no window of versions: it
   *  answers only the update to the newest on its line. */
  older: string;
  olderGo: string;
  asking: string;

  remove: string;
  removeTitle(machine: string): string;
  removeMany(count: number): string;
  /** Said when one who is not its owner takes a machine off: its owner and
   *  everyone it is shared with lose it too. */
  othersLose(count: number): string;
  ways: { uninstall: { title: string; what: string }; everything: { title: string; what: string }; leave: { title: string; what: string } };
  /** Why it cannot uninstall Fastctx from here. */
  notMember: string;
  /** Of several taken off: none is uninstalled from here, or some only
   *  leave, and why when one reason holds for all of them. */
  noneUninstall: string;
  onlyLeave(count: number, why: string | null): string;
  notApp: string;
  /** A managed computer: taken off, it is unlocked and uninstalled at it. */
  managedApp: string;
  notHub: string;
  hubOlder: string;
  removeOffline: string;
  removeGo: string;
  /** Machines that could not be taken off, by name; why is a hover away. */
  removeFailed(machines: string[]): string;
  /** Once taken off, how the uninstall goes on each machine told to: under
   *  way, waiting for one offline, quiet till the end, or not all done, said
   *  in its own words with the line to run there. */
  removedTitle: string;
  uninstalling: string;
  uninstallAway: string;
  uninstallQuiet: string;
  uninstallLeft: string;
  /** Before the line a machine said to run there as an administrator. */
  runThere: string;
  copy: string;
  copied: string;
  done: string;
  /** The machine that runs the network's hub. */
  hubHere: string;
  hubGo: string;
  /** The copies it holds of conversations living elsewhere (`machine-copies.ts`). */
  copies: CopiesWords;
}

export const zhManage: ManageWords = {
  close: "关闭",
  fastctx: "Fastctx",
  withApp: "带应用",
  app: (version) => `应用 ${version}`,
  upToDate: "已是最新",
  canTake: (version) => `可更新到 ${version}`,
  update: "更新…",
  fetching: (received, total, network) => `下载中 ${total ? `${received} / ${total}` : received}${network ? "，经网络" : ""}`,
  phase: {
    staged: "新版本已就绪",
    preparing: "正在准备…",
    waiting_for_idle: "等待当前工作结束…",
    awaiting_administrator: "正在更新开机服务",
    awaiting_legacy_exit: "等待旧版本退出，该机器重启后完成",
    updating_guardian: "正在更新开机服务…",
    returning_to_service: "正在交回开机服务…",
    complete: "已更新",
    failed: "更新失败",
  },
  failed: failure(zhProblems, "更新失败"),
  unread: failure(zhProblems, "无法读取更新状态"),
  appTakes: (version) => `应用运行中，由应用自行更新到 ${version}`,
  appClosed: (version) => `应用未运行，由本机服务安装 ${version}`,
  appOpenThere: (version) => `该机器下次打开 Fastctx 时更新到 ${version}`,
  olderApp: (version) => `在该电脑的 Fastctx 设置中更新到 ${version}`,
  due: (version, by) => `${by} 已安排上线后更新到 ${version}`,
  away: "不在线",
  older: "旧版本只能更新到最新版，不能选择版本",
  olderGo: "更新",
  asking: "正在询问…",

  remove: "移除…",
  removeTitle: (machine) => `移除 ${machine}？`,
  removeMany: (count) => `移除这 ${count} 台机器？`,
  othersLose: (count) => (count === 1 ? "移出后，其主人及所有使用者都将失去此机器" : "移出后，其主人及所有使用者都将失去这些机器"),
  ways: {
    uninstall: { title: "移出并卸载，保留数据", what: "卸载该机器上的 Fastctx，保留世界记录、登录与身份，重新安装后可继续使用；清除本网络同步过去的数据" },
    everything: { title: "移出并彻底移除", what: "清除 Fastctx 在该机器上写入的全部内容" },
    leave: { title: "只移出网络", what: "保留该机器上的 Fastctx，但不再连接本网络；清除本网络同步过去的数据" },
  },
  notMember: "成员的机器由成员自行卸载，此处仅能移出网络",
  noneUninstall: "这些机器均不能在此卸载，仅能移出网络",
  onlyLeave: (count, why) => `其中 ${count} 台仅移出网络${why ? `：${why}` : ""}`,
  notApp: "带应用的电脑需在该电脑的设置中卸载",
  managedApp: "受管电脑移出后，需在该电脑上卸载",
  notHub: "该机器运行着网络的中枢，请先迁移中枢或解散网络",
  hubOlder: "本网络的中枢版本较旧，暂不支持在此卸载",
  removeOffline: "离线：移出立即生效，下次连接时卸载",
  removeGo: "移除",
  removeFailed: (machines) => (machines.length ? `未能移除 ${machines.join("、")}` : "移除失败"),
  removedTitle: "已移出",
  uninstalling: "正在卸载…",
  uninstallAway: "离线：下次连接时卸载",
  uninstallQuiet: "未报告问题",
  uninstallLeft: "卸载未完成",
  runThere: "在该机器上以管理员身份运行",
  copy: "复制",
  copied: "已复制",
  done: "完成",
  hubHere: "该机器运行着网络的中枢：卸载前请先在机器图上迁移中枢或解散网络",
  hubGo: "前往机器图",
  copies: {
    title: "会话副本",
    what: "这台机器为别的机器上的会话存的副本。服务器一般用不到：停收之后，可以把已经存下的清掉。会话本身留在它所在的机器上。",
    takenIn: "接收中",
    paused: "已停收",
    pausedHeld: (count, size) => `停收的副本 ${count} 段，约 ${size}`,
    pause: "停收",
    takeIn: "恢复接收",
    count: "看能清多少",
    clearable: (count, size) => `可以清掉 ${count} 段，约 ${size}`,
    none: "没有可以清的副本",
    clear: (count) => `清掉 ${count} 段`,
    confirmTitle: "清掉会话副本",
    confirm: (account, count, size, machine) =>
      `清掉 ${machine} 上 ${account} 的 ${count} 段会话副本（约 ${size}）？会话本身留在它们所在的机器上；以后在这台机器恢复接收，会整段重新取回。`,
    cleared: (count, size) => `已清掉 ${count} 段，约 ${size}。`,
    kept: (count) => `${count} 段保留（这台机器写过，或正在用）。`,
    compact: "要在那台机器上压缩之后才还出磁盘：fastctx data compact --yes（会中断那台机器上的会话几分钟）",
    asking: "正在问…",
    noAccount: "这台机器上没有登录的账号",
    unread: (why) => `问不到：${why}`,
  },
};

export const enManage: ManageWords = {
  close: "Close",
  fastctx: "Fastctx",
  withApp: "With the app",
  app: (version) => `App ${version}`,
  upToDate: "Up to date",
  canTake: (version) => `${version} available`,
  update: "Update…",
  fetching: (received, total, network) => `Downloading ${total ? `${received} of ${total}` : received}${network ? ", through the network" : ""}`,
  phase: {
    staged: "Update ready",
    preparing: "Preparing…",
    waiting_for_idle: "Waiting for current work to pause…",
    awaiting_administrator: "Updating its startup service",
    awaiting_legacy_exit: "Waiting for the old version to exit; finishes after it restarts",
    updating_guardian: "Updating startup service…",
    returning_to_service: "Handing back to the startup service…",
    complete: "Updated",
    failed: "Update failed",
  },
  failed: failure(enProblems, "Update failed"),
  unread: failure(enProblems, "Its update cannot be read"),
  appTakes: (version) => `Its app is open and updates itself to ${version}`,
  appClosed: (version) => `Its app is closed; the local service installs ${version}`,
  appOpenThere: (version) => `Updates to ${version} when Fastctx next opens there`,
  olderApp: (version) => `Update to ${version} in Fastctx's settings on that computer`,
  due: (version, by) => `${by} set it to update to ${version} once it's back`,
  away: "Offline",
  older: "An older version updates only to the newest; no version can be picked",
  olderGo: "Update",
  asking: "Asking…",

  remove: "Remove…",
  removeTitle: (machine) => `Remove ${machine}?`,
  removeMany: (count) => `Remove these ${count}?`,
  othersLose: (count) => (count === 1 ? "Once it is off, its owner and everyone who uses it no longer have it" : "Once they are off, their owners and everyone who uses them no longer have them"),
  ways: {
    uninstall: { title: "Remove and uninstall, keeping its data", what: "Fastctx goes from it; its World's records, logins and identity stay, to carry on once installed again; what this network synced to it is cleared" },
    everything: { title: "Remove and erase", what: "Everything Fastctx wrote there goes" },
    leave: { title: "Only take it off the network", what: "Fastctx stays on it, no longer reaches this network, and what this network synced to it is cleared" },
  },
  notMember: "A member's machine is the member's to uninstall; here it is only taken off",
  noneUninstall: "None of these can be uninstalled from here; they are only taken off",
  onlyLeave: (count, why) => `${count} of them only leave the network${why ? `: ${why}` : ""}`,
  notApp: "A computer with the app is uninstalled by whoever sits at it, in its own settings",
  managedApp: "Once taken off, a managed computer is uninstalled by whoever sits at it",
  notHub: "It runs the network's hub: move the hub or dissolve the network first",
  hubOlder: "This network's hub is older and cannot uninstall it yet",
  removeOffline: "Offline: it is taken off at once, and uninstalls when it next connects",
  removeGo: "Remove",
  removeFailed: (machines) => (machines.length ? `Could not remove ${machines.join(", ")}` : "Could not remove it"),
  removedTitle: "Taken off",
  uninstalling: "Uninstalling there…",
  uninstallAway: "Offline: uninstalls once it is back",
  uninstallQuiet: "No problem reported",
  uninstallLeft: "Not all uninstalled",
  runThere: "Run on that machine as an administrator",
  copy: "Copy",
  copied: "Copied",
  done: "Done",
  hubHere: "It runs the network's hub: to uninstall it, move the hub or dissolve the network on the machines map first",
  hubGo: "To the map",
  copies: {
    title: "Conversation copies",
    what: "Copies this machine keeps of conversations living on other machines. A server seldom reads them: once paused, what it keeps can be let go. The conversations stay where they live.",
    takenIn: "Taken in",
    paused: "Paused",
    pausedHeld: (count, size) => `${count} paused, about ${size}`,
    pause: "Pause",
    takeIn: "Take in again",
    count: "Count what may go",
    clearable: (count, size) => `${count} may go, about ${size}`,
    none: "Nothing may go",
    clear: (count) => `Let ${count} go`,
    confirmTitle: "Let copies go",
    confirm: (account, count, size, machine) =>
      `Let go ${count} copies of ${account}'s conversations on ${machine} (about ${size})? The conversations stay where they live; taken in again here, they come down whole.`,
    cleared: (count, size) => `${count} let go, about ${size}.`,
    kept: (count) => `${count} kept (written here, or in use).`,
    compact: "The disk gets the room back once that machine's library is compacted there: fastctx data compact --yes (stops its conversations for a few minutes)",
    asking: "Asking…",
    noAccount: "No account is logged in on this machine",
    unread: (why) => `Could not ask: ${why}`,
  },
};
