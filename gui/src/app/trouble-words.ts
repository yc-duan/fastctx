// What the hub tells of what went wrong with a machine or with the network
// (`specs/world-engine.md`, 量与告警), in the interface's words: a short
// phrase for the alerts and a notification, and the detail behind a
// pointer's rest. The hub's bounds are named plainly; one this release does
// not know keeps the name the hub gives it.

import type { Trouble } from "../world/accounts";

export interface TroubleWords {
  /** The machines page's view of them. */
  view: string;
  /** Where none was told within the week. */
  none: string;
  /** The week's ruler: today, and a day by its weekday (0 is Sunday), in
   *  full and by its first letter. */
  today: string;
  weekday(day: number): string;
  weekdayShort(day: number): string;
  /** What happened, short. */
  what(trouble: Trouble): string;
  /** A notification's title: the machine and what happened to it, or what
   *  happened to the network. */
  title(trouble: Trouble): string;
  /** How long, how many, or which bound; empty where there is no more. */
  detail(trouble: Trouble): string;
  /** The bounds the hub names, by their plain names. */
  bounds: Record<string, string>;
}

/** Whether a trouble is a fault, drawn in its colour, or a warning. */
export function troubleTone(kind: Trouble["kind"]): "fault" | "warning" {
  return kind === "silent" || kind === "stuck" || kind === "outage" || kind === "damaged" ? "fault" : "warning";
}

const zhBounds: Record<string, string> = {
  "link.frames": "发往中枢服务器的队列",
  "log.waiting": "日志队列",
  "hub.input": "接收队列",
  "storage.node.writes": "记录的写入队列",
  "storage.book.writes": "活动账本的写入队列",
  "storage.hub.writes": "中枢记录的写入队列",
};
const enBounds: Record<string, string> = {
  "link.frames": "Queue to the hub",
  "log.waiting": "Log queue",
  "hub.input": "Incoming queue",
  "storage.node.writes": "Record write queue",
  "storage.book.writes": "Activity book write queue",
  "storage.hub.writes": "Hub record write queue",
};

/** A bound by its plain name; one this release does not know, as named. */
export const zhBound = (bound: string): string => zhBounds[bound] ?? bound;
export const enBound = (bound: string): string => enBounds[bound] ?? bound;

const zhTitle = (trouble: Trouble, what: string): string => {
  if (!trouble.machine) return what;
  return trouble.kind === "stuck" ? `${trouble.machine} 卡住了` : `${trouble.machine} ${what}`;
};

const enTitle = (trouble: Trouble, what: string): string => {
  const machine = trouble.machine;
  if (!machine) return what;
  switch (trouble.kind) {
    case "silent":
      return `${machine} went silent`;
    case "drops":
      return `${machine} keeps dropping`;
    case "stuck":
      return `${machine} is stuck`;
    case "near":
      return `${machine} is near a limit`;
    case "damaged":
      return `${machine} has damaged records`;
    case "violations":
      return `${machine} is misbehaving`;
    default:
      return `${machine}: ${what.charAt(0).toLowerCase()}${what.slice(1)}`;
  }
};

export const zhTroubles: TroubleWords = {
  view: "告警",
  bounds: zhBounds,
  none: "近 7 天没有告警",
  today: "今天",
  weekday: (day) => `周${"日一二三四五六"[day] ?? ""}`,
  weekdayShort: (day) => "日一二三四五六"[day] ?? "",
  title: (trouble) => zhTitle(trouble, zhTroubles.what(trouble)),
  what: (trouble) => {
    switch (trouble.kind) {
      case "silent":
        return "失去联系";
      case "drops":
        return "频繁断线";
      case "stuck":
        return "卡住";
      case "near":
        return "快到上限";
      case "damaged":
        return "记录损坏";
      case "violations":
        return "运行异常";
      case "outage":
        return trouble.count ? `${trouble.count} 台服务器同时失去联系` : "多台服务器同时失去联系";
      case "hub":
        return trouble.near ? "中枢服务器快到上限" : "中枢服务器很忙";
      case "rollout":
        return trouble.version ? `${trouble.version} 的推广已停下` : "推广已停下";
      default:
        return "出现异常";
    }
  },
  detail: (trouble) => {
    const near = trouble.near ? `${zhBounds[trouble.near.bound] ?? trouble.near.bound}已用 ${trouble.near.used}%` : "";
    switch (trouble.kind) {
      case "silent":
        return trouble.seconds ? `${trouble.seconds} 秒没有回应` : "";
      case "drops":
        return trouble.count ? `一小时内重连 ${trouble.count} 次` : "";
      case "stuck":
        return trouble.seconds ? `有任务 ${trouble.seconds} 秒没有进展` : "";
      case "near":
        return near;
      case "damaged":
        return "部分记录可能已经丢失";
      case "violations":
        return "本机服务持续违反运行规则";
      case "hub":
        return near || (trouble.seconds ? `任务最久等了 ${trouble.seconds} 秒` : "");
      case "rollout":
        return "换上这一版的机器出问题更多";
      default:
        return "";
    }
  },
};

export const enTroubles: TroubleWords = {
  view: "Alerts",
  bounds: enBounds,
  none: "No alerts in the last 7 days",
  today: "Today",
  weekday: (day) => ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][day] ?? "",
  weekdayShort: (day) => "SMTWTFS"[day] ?? "",
  title: (trouble) => enTitle(trouble, enTroubles.what(trouble)),
  what: (trouble) => {
    switch (trouble.kind) {
      case "silent":
        return "Went silent";
      case "drops":
        return "Dropping often";
      case "stuck":
        return "Stuck";
      case "near":
        return "Near a limit";
      case "damaged":
        return "Records damaged";
      case "violations":
        return "Misbehaving";
      case "outage":
        return trouble.count ? `${trouble.count} servers went silent together` : "Servers went silent together";
      case "hub":
        return trouble.near ? "Hub near a limit" : "Hub overloaded";
      case "rollout":
        return trouble.version ? `Rollout of ${trouble.version} stopped` : "Rollout stopped";
      default:
        return "Something went wrong";
    }
  },
  detail: (trouble) => {
    const near = trouble.near ? `${enBounds[trouble.near.bound] ?? trouble.near.bound} ${trouble.near.used}% full` : "";
    switch (trouble.kind) {
      case "silent":
        return trouble.seconds ? `No answer for ${trouble.seconds} s` : "";
      case "drops":
        return trouble.count ? `Reconnected ${trouble.count} times within an hour` : "";
      case "stuck":
        return trouble.seconds ? `Work stalled for ${trouble.seconds} s` : "";
      case "near":
        return near;
      case "damaged":
        return "Some records may be lost";
      case "violations":
        return "Its local service keeps breaking its rules";
      case "hub":
        return near || (trouble.seconds ? `Work waited up to ${trouble.seconds} s` : "");
      case "rollout":
        return "Machines on it ran into more trouble than the rest";
      default:
        return "";
    }
  },
};
