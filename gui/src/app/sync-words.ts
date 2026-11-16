// The words of the sync status page (`sync-page.ts`) and of its way in.

import type { Part } from "../world/sync";

export interface SyncWords {
  title: string;
  /** The span of the line: a day, a week. */
  spans: { day: string; week: string };
  spanLabel: string;
  /** At the top: all caught up, or how many of how many. */
  allSynced(devices: number): string;
  someSynced(synced: number, devices: number): string;
  /** None of the account's machines told the hub where it stands yet. */
  noneReported: string;
  /** This computer, beside its name. */
  thisOne: string;
  kinds: { computer: string; phone: string; other: string };
  /** A card's state. */
  synced: string;
  textSynced(files: number): string;
  catching(left: string): string;
  eta(left: string): string;
  away(since: string, left: string | null): string;
  /** Offline with files to catch up on and no text, or what is left not
   *  known. */
  awayFiles(since: string, files: number): string;
  awayUnknown(since: string): string;
  neverReported: string;
  behindBy(behind: string): string;
  unpulled(scopes: number): string;
  files(count: number, size: string): string;
  unsent(frames: number, size: string): string;
  parts: Record<Part, string>;
  filesPart: string;
  /** The line of how far behind each machine stood. */
  lineTitle: string;
  lineSaid: string;
  /** No point in the span. */
  lineEmpty: string;
  offline: string;
  reconnected: string;
  caughtUp: string;
  at(at: Date, span: "day" | "week"): string;
  axisTime(at: Date, span: "day" | "week"): string;
  /** What the hub holds of the account. */
  hubTitle: string;
  /** What the hub's totals count. */
  hubSaid: string;
  conversations(n: number): string;
  hubFiles(n: number, size: string): string;
  records(size: string): string;
  /** This computer's own: what it has not sent, and how long it has been
   *  cut off. */
  hereTitle: string;
  nothingUnsent: string;
  cutOff(since: string): string;
  unreached: string;
  reading: string;
  failed: string;
  /** How long, as a duration. */
  duration(ms: number): string;
  /** Between this computer and the hub: the two ways, and what each says. */
  up: string;
  down: string;
  here: string;
  hub: string;
  toUpload(frames: number, size: string): string;
  /** Said of the way down while the two are cut off. */
  resumes: string;
  /** What a scope a device has not pulled whole is, said to the account
   *  it is of. */
  scope: { talk(name: string): string; records: string; shared: string; project(name: string): string; working: string; hub: string };
  more(n: number): string;
  /** The list of devices: how many, the standings it is sorted by, the
   *  search, the heads over each standing's columns, and the row that
   *  stands for the rest of a long group. */
  devices(n: number): string;
  standings: { all: string; syncing: string; silent: string; away: string; synced: string };
  find: string;
  heads: { left: string; behind: string; away: string; onReturn: string; recent(span: "day" | "week"): string };
  rest(n: number): string;
  noMatch: string;
  nothingMissed: string;
  silentSaid: string;
  /** A device opened: what it is doing and why, in a sentence, and the
   *  facts under it. */
  whySyncing(left: string, eta: string | null): string;
  whyBehind(behind: string): string;
  whyFiles(files: number, size: string): string;
  whySynced(ago: string): string;
  whySilent: string;
  facts: { download: string; files: string; unpulled: string; upload: string; rate: string; online: string; offline: string; reported: string };
  fileCount(count: number, size: string): string;
  frames(frames: number, size: string): string;
  scopes(n: number): string;
  perSecond(size: string): string;
  ago(since: string): string;
  /** The way in, on 「我」 and beside it in the sidebar. */
  entry: string;
  entrySaid(synced: number, devices: number): string;
}

const zhDuration = (ms: number): string => {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return seconds <= 1 ? "1 秒" : `${seconds} 秒`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} 小时`;
  return `${Math.round(hours / 24)} 天`;
};

const enDuration = (ms: number): string => {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return seconds <= 1 ? "1 second" : `${seconds} seconds`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return minutes === 1 ? "1 minute" : `${minutes} minutes`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return hours === 1 ? "1 hour" : `${hours} hours`;
  const days = Math.round(hours / 24);
  return `${days} days`;
};

const pad = (n: number) => String(n).padStart(2, "0");

export const zhSync: SyncWords = {
  title: "同步",
  spans: { day: "24 小时", week: "7 天" },
  spanLabel: "时间范围",
  allSynced: (devices) => (devices === 1 ? "本机已同步" : `${devices} 台均已同步`),
  someSynced: (synced, devices) => `${devices} 台中 ${synced} 台已同步`,
  noneReported: "尚无设备报告同步进度",
  thisOne: "本机",
  kinds: { computer: "电脑", phone: "手机", other: "设备" },
  synced: "已同步",
  textSynced: (files) => `文字已同步，文件待同步 ${files} 个`,
  catching: (left) => `同步中，剩余 ${left}`,
  eta: (left) => `约 ${left}`,
  away: (since, left) => (left ? `离线 ${since}，恢复后约需同步 ${left}` : `离线 ${since}，无遗漏`),
  awayFiles: (since, files) => `离线 ${since}，恢复后需同步 ${files} 个文件`,
  awayUnknown: (since) => `离线 ${since}，恢复后需继续同步`,
  neverReported: "尚未报告同步进度",
  behindBy: (behind) => `落后 ${behind}`,
  unpulled: (scopes) => `${scopes} 段尚未取全`,
  files: (count, size) => `缺少 ${count} 个文件，${size}`,
  unsent: (frames, size) => `${frames} 条未上传，${size}`,
  parts: { conversations: "对话", output: "作业输出", other: "其余" },
  filesPart: "文件",
  lineTitle: "落后程度",
  lineSaid: "各设备落后于中枢最新变更的时长；离线时段留空",
  lineEmpty: "此时段暂无记录",
  offline: "离线",
  reconnected: "已重新连接",
  caughtUp: "同步完成",
  at: (at, span) => (span === "day" ? `${pad(at.getHours())}:${pad(at.getMinutes())}` : `${at.getMonth() + 1} 月 ${at.getDate()} 日 ${pad(at.getHours())}:00`),
  axisTime: (at, span) => (span === "day" ? `${pad(at.getHours())}:00` : ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][at.getDay()]!),
  hubTitle: "中枢",
  hubSaid: "此账号及所属共有体在中枢存储的合计",
  conversations: (n) => `${n} 段对话`,
  hubFiles: (n, size) => `${n} 个文件，${size}`,
  records: (size) => `文字 ${size}`,
  hereTitle: "本机",
  nothingUnsent: "均已上传",
  cutOff: (since) => `与中枢断开 ${since}`,
  unreached: "无法连接中枢，仅显示本机记录",
  reading: "正在查询中枢",
  failed: "读取失败",
  duration: zhDuration,
  up: "上传",
  down: "下载",
  here: "本机",
  hub: "中枢",
  toUpload: (frames, size) => `${frames} 条待上传，${size}`,
  resumes: "连接恢复后继续",
  scope: { talk: (name) => `对话「${name}」`, records: "账号记录", shared: "共享机器的记录", project: (name) => `项目 ${name}`, working: "工作副本", hub: "网络记录" },
  more: (n) => `等 ${n} 段`,
  devices: (n) => `${n} 台设备`,
  standings: { all: "全部", syncing: "同步中", silent: "未报告", away: "离线", synced: "已同步" },
  find: "搜索设备",
  heads: { left: "剩余", behind: "落后", away: "离线", onReturn: "恢复后需同步", recent: (span) => (span === "day" ? "近 24 小时" : "近 7 天") },
  rest: (n) => `其余 ${n} 台`,
  noMatch: "无匹配设备",
  nothingMissed: "无遗漏",
  silentSaid: "旧版本的设备不报告同步进度，升级后开始同步",
  whySyncing: (left, eta) => `正在从中枢下载，剩余 ${left}${eta ? `，${eta}` : ""}`,
  whyBehind: (behind) => `落后中枢 ${behind}，正在追平`,
  whyFiles: (files, size) => `文字已同步，${files} 个文件待下载，${size}`,
  whySynced: (ago) => `已同步，${ago}确认`,
  whySilent: "已登录此账号，尚未报告同步进度。旧版本的设备不参与同步，升级后开始",
  facts: { download: "待下载", files: "文件", unpulled: "未取全", upload: "待上传", rate: "速度", online: "在线", offline: "离线", reported: "最近报告" },
  fileCount: (count, size) => `${count} 个，${size}`,
  frames: (frames, size) => `${frames} 条，${size}`,
  scopes: (n) => `${n} 段`,
  perSecond: (size) => `${size}/s`,
  ago: (since) => `${since}前`,
  entry: "同步",
  entrySaid: (synced, devices) => (synced === devices ? (devices === 1 ? "已齐" : `${devices} 台都已齐`) : `${devices} 台中 ${synced} 台已同步`),
};

export const enSync: SyncWords = {
  title: "Sync",
  spans: { day: "24 hours", week: "7 days" },
  spanLabel: "Span",
  allSynced: (devices) => (devices === 1 ? "This device is caught up" : `All ${devices} devices are caught up`),
  someSynced: (synced, devices) => `${synced} of ${devices} devices caught up`,
  noneReported: "No device has told where it stands yet",
  thisOne: "This one",
  kinds: { computer: "Computer", phone: "Phone", other: "Device" },
  synced: "Caught up",
  textSynced: (files) => `Text caught up, ${files} ${files === 1 ? "file" : "files"} to come`,
  catching: (left) => `Catching up, ${left} to go`,
  eta: (left) => `about ${left}`,
  away: (since, left) => (left ? `Offline ${since}, about ${left} to catch up on return` : `Offline ${since}, nothing missed`),
  awayFiles: (since, files) => `Offline ${since}, ${files} ${files === 1 ? "file" : "files"} to catch up on return`,
  awayUnknown: (since) => `Offline ${since}, more to catch up on return`,
  neverReported: "Has not told where it stands",
  behindBy: (behind) => `${behind} behind the latest`,
  unpulled: (scopes) => `${scopes} not taken whole yet`,
  files: (count, size) => `${count} ${count === 1 ? "file" : "files"} missing, ${size}`,
  unsent: (frames, size) => `${frames} not sent yet, ${size}`,
  parts: { conversations: "Conversations", output: "Job output", other: "Other" },
  filesPart: "Files",
  lineTitle: "How far behind",
  lineSaid: "How far each device stood behind the hub's latest change; offline stretches are left blank",
  lineEmpty: "Nothing recorded in this span yet",
  offline: "offline",
  reconnected: "reconnected",
  caughtUp: "caught up",
  at: (at, span) => (span === "day" ? `${pad(at.getHours())}:${pad(at.getMinutes())}` : `${at.toLocaleDateString("en", { month: "short", day: "numeric" })} ${pad(at.getHours())}:00`),
  axisTime: (at, span) => (span === "day" ? `${pad(at.getHours())}:00` : at.toLocaleDateString("en", { weekday: "short" })),
  hubTitle: "On the hub",
  hubSaid: "What the hub keeps of this account and of the bodies it is a member of, together",
  conversations: (n) => `${n} ${n === 1 ? "conversation" : "conversations"}`,
  hubFiles: (n, size) => `${n} ${n === 1 ? "file" : "files"}, ${size}`,
  records: (size) => `${size} of text`,
  hereTitle: "This computer",
  nothingUnsent: "Everything sent",
  cutOff: (since) => `Cut off from the hub for ${since}`,
  unreached: "The hub is not reached; this is what this computer knows itself",
  reading: "Asking the hub",
  failed: "Could not read it",
  duration: enDuration,
  up: "Up",
  down: "Down",
  here: "This device",
  hub: "Hub",
  toUpload: (frames, size) => `${frames} to send, ${size}`,
  resumes: "Goes on once connected",
  scope: { talk: (name) => `the conversation “${name}”`, records: "the account's records", shared: "a shared machine's records", project: (name) => `the project ${name}`, working: "a working copy", hub: "the network's records" },
  more: (n) => `and ${n} more`,
  devices: (n) => `${n} ${n === 1 ? "device" : "devices"}`,
  standings: { all: "All", syncing: "Catching up", silent: "Not reporting", away: "Offline", synced: "Caught up" },
  find: "Find a device",
  heads: { left: "To go", behind: "Behind", away: "Offline", onReturn: "To catch up on return", recent: (span) => (span === "day" ? "Last 24 hours" : "Last 7 days") },
  rest: (n) => `${n} more`,
  noMatch: "No device matches",
  nothingMissed: "Nothing missed",
  silentSaid: "A device on an earlier version does not tell where it stands; it syncs once updated",
  whySyncing: (left, eta) => `Taking from the hub, ${left} to go${eta ? `, ${eta}` : ""}`,
  whyBehind: (behind) => `${behind} behind the hub, catching up`,
  whyFiles: (files, size) => `Text caught up; ${files} ${files === 1 ? "file" : "files"} to come, ${size}`,
  whySynced: (ago) => `Caught up, confirmed ${ago}`,
  whySilent: "Logged in to this account, it has not told where it stands. A device on an earlier version takes no part in syncing until it is updated",
  facts: { download: "To take", files: "Files", unpulled: "Not whole yet", upload: "To send", rate: "Rate", online: "Online", offline: "Offline", reported: "Last told" },
  fileCount: (count, size) => `${count}, ${size}`,
  frames: (frames, size) => `${frames}, ${size}`,
  scopes: (n) => `${n}`,
  perSecond: (size) => `${size}/s`,
  ago: (since) => `${since} ago`,
  entry: "Sync",
  entrySaid: (synced, devices) => (synced === devices ? (devices === 1 ? "Caught up" : `All ${devices} caught up`) : `${synced} of ${devices} caught up`),
};
