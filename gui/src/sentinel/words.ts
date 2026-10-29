// The words of a watcher's own page (`page.ts`): few, since the strip says
// how it watched and the reading column what it did.

import type { SentinelPhase } from "../world/sentinels";

export interface SentinelWords {
  title: string;
  phase: Record<SentinelPhase, string>;
  withdraw: string;
  withdrawTitle: string;
  withdrawWhat: string;
  reading: string;
  nothing: string;
  earlier: string;
  unreadable: string;
  /** A moment on the strip, pointed at: when it was. */
  at(time: number, now: number): string;
  /** When it next looks or is woken. */
  next(time: number, now: number): string;
  /** How often its reflex looks, for the stretch pointed at. */
  every(ms: number): string;
  /** The page of every watcher (`overview.ts`): its name, its two parts,
   *  the spans it is drawn over, what all of them took in that span, and
   *  one's own, pointed at. */
  overview: {
    title: string;
    watching: string;
    ended: string;
    span: Record<"week" | "month", string>;
    wakes(count: number): string;
    tokens(count: string): string;
    took(wakes: number, tokens: string, time: string): string;
    nothing: string;
    unreachable: string;
  };
}

const time = (locale: string, tomorrow: string) => (at: number, now: number) => {
  const day = new Date(at);
  const today = new Date(now);
  const clock = day.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", hour12: false });
  if (day.toDateString() === today.toDateString()) return clock;
  const next = new Date(now);
  next.setDate(next.getDate() + 1);
  if (day.toDateString() === next.toDateString()) return `${tomorrow} ${clock}`;
  return `${day.toLocaleDateString(locale, { month: "numeric", day: "numeric" })} ${clock}`;
};

const zhTime = time("zh-CN", "明天");
const enTime = time("en-US", "tomorrow");

export const zhSentinel: SentinelWords = {
  title: "观测",
  phase: { starting: "启动中", watching: "观测中", satisfied: "已成立", unable: "无法观测", withdrawn: "已撤销", unavailable: "暂不可达" },
  withdraw: "撤销",
  withdrawTitle: "撤销这项观测？",
  withdrawWhat: "撤销后不再巡检，也不再唤醒对话；已有记录保留。",
  reading: "正在读取…",
  nothing: "尚无记录",
  earlier: "更早的记录未列出",
  unreadable: "无法读取记录",
  at: zhTime,
  next: (at, now) => `下次 ${zhTime(at, now)}`,
  every: (ms) => (ms < 60_000 ? `每 ${Math.round(ms / 1000)} 秒巡检一次` : ms < 3_600_000 ? `每 ${Math.round(ms / 60_000)} 分钟巡检一次` : `每 ${Math.round(ms / 3_600_000)} 小时巡检一次`),
  overview: {
    title: "全部观测",
    watching: "观测中",
    ended: "已结束",
    span: { week: "7 天", month: "30 天" },
    wakes: (count) => `醒来 ${count} 次`,
    tokens: (count) => `${count} tokens`,
    took: (wakes, tokens, time) => `醒来 ${wakes} 次，${tokens} tokens，共 ${time}`,
    nothing: "尚无观测",
    unreachable: "有观测所在的机器暂时读不到",
  },
};

export const enSentinel: SentinelWords = {
  title: "Watcher",
  phase: { starting: "starting", watching: "watching", satisfied: "found", unable: "unable to watch", withdrawn: "withdrawn", unavailable: "unreachable" },
  withdraw: "Withdraw",
  withdrawTitle: "Withdraw this watcher?",
  withdrawWhat: "It stops checking and no longer wakes the conversation; its record is kept.",
  reading: "Reading…",
  nothing: "No record yet",
  earlier: "Earlier records are not listed",
  unreadable: "The record cannot be read",
  at: enTime,
  next: (at, now) => `Next ${enTime(at, now)}`,
  every: (ms) =>
    ms < 60_000 ? `Checks every ${Math.round(ms / 1000)} s` : ms < 3_600_000 ? `Checks every ${Math.round(ms / 60_000)} min` : `Checks every ${Math.round(ms / 3_600_000)} h`,
  overview: {
    title: "All watchers",
    watching: "Watching",
    ended: "Ended",
    span: { week: "7 days", month: "30 days" },
    wakes: (count) => `${count} wake${count === 1 ? "" : "s"}`,
    tokens: (count) => `${count} tokens`,
    took: (wakes, tokens, time) => `${wakes} wake${wakes === 1 ? "" : "s"}, ${tokens} tokens, ${time} in all`,
    nothing: "No watchers yet",
    unreachable: "A machine some watchers live on can't be read now",
  },
};
