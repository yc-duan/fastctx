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
  unreached: "无法连接中枢，以下仅为本机记录",
  reading: "正在查询中枢",
  failed: "读取失败",
  duration: zhDuration,
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
  unreached: "The hub is not reached; below is what this computer knows itself",
  reading: "Asking the hub",
  failed: "Could not read it",
  duration: enDuration,
  entry: "Sync",
  entrySaid: (synced, devices) => (synced === devices ? (devices === 1 ? "Caught up" : `All ${devices} caught up`) : `${synced} of ${devices} caught up`),
};
