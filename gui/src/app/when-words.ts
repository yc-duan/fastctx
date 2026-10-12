// How a moment ahead is said (`when.ts`): in full where it is picked or
// read, short on a tag, and how long until it.

import type { WhenWords } from "./when";

/** What is said of a moment ahead, besides the picker's own words. */
export interface MomentWords {
  /** "until" and the moment in full. */
  until(at: Date, now: Date): string;
  /** The moment on a tag: the day, or the time when it is within two days. */
  short(at: Date, now: Date): string;
  /** The moment in a record: date and time, the year when not this one. */
  stamp(at: Date): string;
}

const clock = (at: Date) => `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;

/** Whole days from `now`'s day to `at`'s. */
function daysAhead(at: Date, now: Date): number {
  const day = (date: Date) => Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
  return Math.round((day(at) - day(now)) / 86_400_000);
}

/** Minutes, hours and days until `at`, the two largest that are not zero. */
function parts(at: Date, now: Date): { days: number; hours: number; minutes: number } {
  const minutes = Math.max(1, Math.ceil((at.getTime() - now.getTime()) / 60_000));
  return { days: Math.floor(minutes / 1440), hours: Math.floor(minutes / 60) % 24, minutes: minutes % 60 };
}

const zhWeek = ["日", "一", "二", "三", "四", "五", "六"];

function zhDay(at: Date, now: Date): string {
  const ahead = daysAhead(at, now);
  if (ahead === 0) return "今天";
  if (ahead === 1) return "明天";
  if (ahead === 2) return "后天";
  const date = `${at.getMonth() + 1}月${at.getDate()}日`;
  return at.getFullYear() === now.getFullYear() ? date : `${at.getFullYear()}年${date}`;
}

function zhAt(at: Date, now: Date): string {
  const near = daysAhead(at, now) <= 2;
  return `${zhDay(at, now)}${near ? "" : ` 周${zhWeek[at.getDay()]}`} ${clock(at)}`;
}

export const zhWhen: WhenWords = {
  none: "不限期",
  pick: "定截止时间",
  month: (year, month) => `${year}年${month + 1}月`,
  weekdays: ["一", "二", "三", "四", "五", "六", "日"],
  earlier: "上个月",
  later: "下个月",
  hour: "时",
  minute: "分",
  at: zhAt,
  left: (at, now) => {
    const { days, hours, minutes } = parts(at, now);
    if (days) return `还有 ${days} 天${hours ? ` ${hours} 小时` : ""}`;
    if (hours) return `还有 ${hours} 小时${minutes ? ` ${minutes} 分钟` : ""}`;
    return `还有 ${minutes} 分钟`;
  },
};

export const zhMoment: MomentWords = {
  until: (at, now) => `到 ${zhAt(at, now)}`,
  short: (at, now) => (daysAhead(at, now) <= 2 ? `${zhDay(at, now)} ${clock(at)}` : zhDay(at, now)),
  stamp: (at) => `${at.getFullYear() === new Date().getFullYear() ? "" : `${at.getFullYear()}年`}${at.getMonth() + 1}月${at.getDate()}日 ${clock(at)}`,
};

function enDay(at: Date, now: Date): string {
  const ahead = daysAhead(at, now);
  if (ahead === 0) return "today";
  if (ahead === 1) return "tomorrow";
  const options: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };
  if (at.getFullYear() !== now.getFullYear()) options.year = "numeric";
  return at.toLocaleDateString("en", options);
}

function enAt(at: Date, now: Date): string {
  if (daysAhead(at, now) <= 1) return `${enDay(at, now)} ${clock(at)}`;
  return `${at.toLocaleDateString("en", { weekday: "short" })}, ${enDay(at, now)}, ${clock(at)}`;
}

export const enWhen: WhenWords = {
  none: "No end",
  pick: "Set an end",
  month: (year, month) => new Date(year, month, 1).toLocaleDateString("en", { month: "long", year: "numeric" }),
  weekdays: ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"],
  earlier: "Previous month",
  later: "Next month",
  hour: "Hour",
  minute: "Minute",
  at: enAt,
  left: (at, now) => {
    const { days, hours, minutes } = parts(at, now);
    if (days) return `${days} d${hours ? ` ${hours} h` : ""} left`;
    if (hours) return `${hours} h${minutes ? ` ${minutes} min` : ""} left`;
    return `${minutes} min left`;
  },
};

export const enMoment: MomentWords = {
  until: (at, now) => `until ${enAt(at, now)}`,
  short: (at, now) => (daysAhead(at, now) <= 1 ? `${enDay(at, now)} ${clock(at)}` : enDay(at, now)),
  stamp: (at) => {
    const options: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };
    if (at.getFullYear() !== new Date().getFullYear()) options.year = "numeric";
    return `${at.toLocaleDateString("en", options)} ${clock(at)}`;
  },
};
