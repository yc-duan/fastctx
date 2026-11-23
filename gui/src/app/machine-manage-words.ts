// The words of a machine's 「管理」 (`machine-fastctx.ts`, `machine-remove.ts`):
// the Fastctx on it, how an update of it goes, and taking it off the network
// three ways.

import type { RetentionWords } from "./machine-retention";
import type { RenewalPhase } from "../world/renewal";
import type { CopiesWords } from "./machine-copies";
import { enProblems, failure, zhProblems } from "./problem";

export interface ManageWords {
  close: string;
  fastctx: string;
  /** Its row: the release it runs, how it runs, and its update. */
  version: string;
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
  retention: RetentionWords;
}

export const zhManage: ManageWords = {
  close: "关闭",
  fastctx: "Fastctx",
  version: "版本",
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
  retention: {
    title: "对话保留",
    own: "我的对话",
    ownWhat: "闲置这么久后连同它的命令输出、附件与宿主记录一起删除",
    assisted: "受协助账号的对话",
    assistedWhat: "最多保留这么久；受协助的人可以给自己设得更短",
    off: "不删除",
    days: (n) => `${n} 天`,
    graceUntil: (when) => `规则刚打开或改短：${when} 之前不删除任何对话。`,
    dueSoon: (count) => `${count} 段对话将在 7 天内到期。`,
    confirmTitle: "闲置的对话将被删除",
    confirm: (days, count, machine) =>
      `${machine} 上闲置超过 ${days} 天的对话会被逐段删除，连同命令输出、附件与宿主记录，无法恢复。7 天之后开始，届时约有 ${count} 段符合条件；这期间说一句话就会重新计时。`,
    confirmGo: "确定",
    asking: "读取中…",
    unread: (why) => `读不到：${why}`,
  },
  copies: {
    title: "会话副本",
    takeIn: "接收会话副本",
    takeInOf: (account) => `接收 ${account} 的会话副本`,
    what: "替别的机器上的会话存一份，方便在这台查看。服务器一般用不到。",
    takenIn: "接收中",
    paused: "已停收",
    pause: "停收",
    resume: "恢复接收",
    held: "已存的副本",
    heldSize: (count, size) => `${count} 段，约 ${size}`,
    clear: "清理…",
    none: "没有可清的",
    confirmTitle: "清理会话副本",
    confirm: (account, count, size, machine) => `清掉 ${machine} 上 ${account} 的 ${count} 段副本，约 ${size}。会话本身不受影响；以后恢复接收会重新取回。`,
    clearGo: (count) => `清掉 ${count} 段`,
    cleared: "已清理",
    clearedSaid: (count, size) => `清掉 ${count} 段，约 ${size}。`,
    kept: (count) => `${count} 段留着（这台写过或正在用）。`,
    stopped: (why) => `中途停了：${why}`,
    compact: "磁盘空间要等这台机器压缩库之后才腾出（fastctx data compact --yes，会暂停会话几分钟）。",
    asking: "正在读取…",
    noAccount: "这台机器没有登录账号",
    unread: (why) => `读不到：${why}`,
  },
};

export const enManage: ManageWords = {
  close: "Close",
  fastctx: "Fastctx",
  version: "Version",
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
  retention: {
    title: "Keeping conversations",
    own: "My conversations",
    ownWhat: "Idle this long, one is deleted with its commands' output, its attachments and its agent's record",
    assisted: "Assisted accounts' conversations",
    assistedWhat: "Kept this long at most; each assisted account may set less for its own",
    off: "Keep",
    days: (n) => `${n} days`,
    graceUntil: (when) => `Just turned on or shortened: nothing is deleted before ${when}.`,
    dueSoon: (count) => `${count} conversation(s) go within 7 days.`,
    confirmTitle: "Idle conversations will be deleted",
    confirm: (days, count, machine) =>
      `Conversations idle more than ${days} days on ${machine} will be deleted one by one, with their commands' output, attachments and agent's record, for good. It starts in 7 days, when about ${count} will be idle that long; a word in one before then starts its count again.`,
    confirmGo: "Turn on",
    asking: "Reading…",
    unread: (why) => `Could not read: ${why}`,
  },
  copies: {
    title: "Conversation copies",
    takeIn: "Keep copies of conversations",
    takeInOf: (account) => `Keep copies of ${account}'s conversations`,
    what: "A copy of conversations living on other machines, to read them here. A server seldom needs it.",
    takenIn: "On",
    paused: "Paused",
    pause: "Pause",
    resume: "Resume",
    held: "Copies kept",
    heldSize: (count, size) => `${count}, about ${size}`,
    clear: "Clear…",
    none: "Nothing to clear",
    confirmTitle: "Clear conversation copies",
    confirm: (account, count, size, machine) =>
      `Clear ${count} copies of ${account}'s conversations on ${machine}, about ${size}. The conversations are not touched; resumed, the copies come back.`,
    clearGo: (count) => `Clear ${count}`,
    cleared: "Cleared",
    clearedSaid: (count, size) => `${count} cleared, about ${size}.`,
    kept: (count) => `${count} kept (written or in use here).`,
    stopped: (why) => `It stopped part way: ${why}`,
    compact: "The disk gets the room back once this machine's library is compacted (fastctx data compact --yes, which pauses its conversations for a few minutes).",
    asking: "Reading…",
    noAccount: "No account is logged in on this machine",
    unread: (why) => `Could not read: ${why}`,
  },
};
