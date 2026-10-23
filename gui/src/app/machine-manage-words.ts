// The words of a machine's 「管理」 (`machine-fastctx.ts`, `machine-remove.ts`):
// the Fastctx on it, how an update of it goes, and taking it off the network
// three ways.

import type { RenewalPhase } from "../world/renewal";

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
  failed(why: string): string;
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
  removeFailed(why: string): string;
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
    waiting_for_idle: "等待当前工作告一段落…",
    awaiting_administrator: "正在更新开机服务",
    awaiting_legacy_exit: "等待旧版本退出，那台重启后完成",
    updating_guardian: "正在更新开机服务…",
    returning_to_service: "正在交回开机服务…",
    complete: "已更新",
    failed: "更新失败",
  },
  failed: (why) => `更新失败：${why}`,
  appTakes: (version) => `应用开着，由应用自己更新到 ${version}`,
  appClosed: (version) => `应用没开，由本机服务装上 ${version}`,
  appOpenThere: (version) => `下次在那台打开 Fastctx 时更新到 ${version}`,
  olderApp: (version) => `在那台电脑的 Fastctx 设置里更新到 ${version}`,
  due: (version, by) => `${by} 安排了上线后更新到 ${version}`,
  away: "不在线",
  older: "旧版本只能直接更新到最新，不能挑版本",
  olderGo: "更新",
  asking: "正在询问…",

  remove: "移除…",
  removeTitle: (machine) => `移除 ${machine}？`,
  removeMany: (count) => `移除这 ${count} 台？`,
  othersLose: (count) => (count === 1 ? "移出之后，它的主人和所有能用它的人都不再有这台" : "移出之后，它们的主人和所有能用它们的人都不再有这些机器"),
  ways: {
    uninstall: { title: "移出并卸载，保留数据", what: "那台上的 Fastctx 卸掉，世界的记录、登录与身份留着，重新装上还能接着用" },
    everything: { title: "移出并彻底移除", what: "把 Fastctx 写在那台上的一切清掉" },
    leave: { title: "只移出网络", what: "那台上的 Fastctx 留着，只是连不上这个网络" },
  },
  notMember: "成员的机器由成员自己卸载，这里只能移出网络",
  noneUninstall: "这几台都不能从这里卸载，只能移出网络",
  onlyLeave: (count, why) => `其中 ${count} 台只移出网络${why ? `：${why}` : ""}`,
  notApp: "带应用的电脑由坐在它前面的人在它自己的设置里卸载",
  managedApp: "受管电脑移出之后，由坐在它前面的人在那台上卸载",
  notHub: "这台跑着网络的中枢，先搬走中枢或解散网络",
  hubOlder: "这个网络的中枢版本较旧，还不能替它卸载",
  removeOffline: "不在线：移出立即生效，卸载在它下次连上时进行",
  removeGo: "移除",
  removeFailed: (why) => `没能移除：${why}`,
  removedTitle: "已移出",
  uninstalling: "正在那台上卸载…",
  uninstallAway: "不在线：下次连上时卸载",
  uninstallQuiet: "没有报告问题",
  uninstallLeft: "没卸完",
  runThere: "在那台上以管理员身份运行",
  copy: "复制",
  copied: "已复制",
  done: "完成",
  hubHere: "这台跑着网络的中枢：要卸载它，先在机器图上搬走中枢或解散网络",
  hubGo: "到机器图上",
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
  failed: (why) => `Update failed: ${why}`,
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
    uninstall: { title: "Remove and uninstall, keeping its data", what: "Fastctx goes from it; its World's records, logins and identity stay, to carry on once installed again" },
    everything: { title: "Remove and erase", what: "Everything Fastctx wrote there goes" },
    leave: { title: "Only take it off the network", what: "Fastctx stays on it, and no longer reaches this network" },
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
  removeFailed: (why) => `Couldn't remove it: ${why}`,
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
};
