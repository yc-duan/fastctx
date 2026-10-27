// The words of the panel beside a conversation, the goal's bar above the
// input, and the pages the panel holds.

import type { GoalStatus, NodeState, PausedBy } from "./types";

export interface SideWords {
  pages: { plan: string; goal: string; collab: string };
  /** The head's switch for the panel. */
  panel: string;
  close: string;
  back: string;
  goal: GoalWords;
  collab: CollabWords;
}

export interface GoalWords {
  /** The bar's name of a goal in each state. */
  bar: Record<GoalStatus, string>;
  /** The page's word for each state. */
  states: Record<GoalStatus, string>;
  paused: (by: PausedBy, until: string | null) => string;
  pause: string;
  resume: string;
  clear: string;
  expand: string;
  cleared: string;
  undo: string;
  /** What the user did to the goal did not go through. */
  failed: string;
  /** The goal changed elsewhere before what the user did reached it. */
  changed: string;
  /** The empty field's placeholder, and its act. */
  ask: string;
  start: string;
  save: string;
  /** The agent's account of a goal done or blocked. */
  done: string;
  blocked: string;
  /** The goal's sentinel, last it looked. */
  sentinel: string;
  sentinelAt: (at: string) => string;
  looked: string;
  saw: string;
  thinks: string;
  worked: (ms: number) => string;
  /** A moment, said as a clock's time, or with its day when not today. */
  at: (ms: number, now: number) => string;
}

export interface CollabWords {
  empty: string;
  here: string;
  states: Record<NodeState, string>;
  goal: Record<GoalStatus, string>;
  steps: (n: number) => string;
  /** Between the facts of one line said when pointed at. */
  separator: string;
  readOnly: string;
  back: string;
  /** To the row of the call that sent one of the host's agents. */
  jump: string;
}

function clock(ms: number, now: number, locale: string, today: string, tomorrow: string): string {
  const day = new Date(ms);
  const time = day.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", hour12: false });
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  const days = Math.floor((ms - start.getTime()) / 86_400_000);
  if (days === 0) return today ? `${today} ${time}` : time;
  if (days === 1) return `${tomorrow} ${time}`;
  return `${day.toLocaleDateString(locale, { month: "short", day: "numeric" })} ${time}`;
}

export const zhSide: SideWords = {
  pages: { plan: "计划", goal: "目标", collab: "协作" },
  panel: "侧面板",
  close: "收起",
  back: "返回",
  goal: {
    bar: { active: "进行中的目标", paused: "已暂停", blocked: "阻碍", complete: "已完成" },
    states: { active: "进行中", paused: "已暂停", blocked: "阻碍", complete: "已完成" },
    paused: (by, until) =>
      by === "quota" ? (until ? `额度用完，${until} 接着` : "额度用完") : by === "world" ? "停下了，等你定" : by === "agent" ? "照你的话停下" : "你停下的",
    pause: "暂停",
    resume: "接着",
    clear: "清掉",
    expand: "展开",
    cleared: "目标已清掉",
    undo: "撤回",
    failed: "目标没改成",
    changed: "目标在别处刚改过，看过再改",
    ask: "要它做成什么",
    start: "设为目标",
    save: "保存",
    done: "完成说明",
    blocked: "阻碍说明",
    sentinel: "哨兵",
    sentinelAt: (at) => `${at} 看过`,
    looked: "看了",
    saw: "看到",
    thinks: "认为",
    worked: (ms) => {
      const minutes = Math.floor(ms / 60_000);
      if (minutes < 1) return "不到 1 分";
      if (minutes < 60) return `${minutes} 分`;
      const hours = Math.floor(minutes / 60);
      if (hours < 48) return minutes % 60 ? `${hours} 小时 ${minutes % 60} 分` : `${hours} 小时`;
      return hours % 24 ? `${Math.floor(hours / 24)} 天 ${hours % 24} 小时` : `${Math.floor(hours / 24)} 天`;
    },
    at: (ms, now) => clock(ms, now, "zh-CN", "", "明天"),
  },
  collab: {
    empty: "还没有同事或子代理",
    here: "这段对话",
    states: { working: "在干", done: "交回了", failed: "失败", stopped: "已停下", unknown: "不知道是否还在干" },
    goal: { active: "进行中", paused: "已暂停", blocked: "阻碍", complete: "已完成" },
    steps: (n) => `${n} 步`,
    separator: "，",
    readOnly: "只读",
    back: "协作",
    jump: "转到它",
  },
};

export const enSide: SideWords = {
  pages: { plan: "Plan", goal: "Goal", collab: "Team" },
  panel: "Side panel",
  close: "Close",
  back: "Back",
  goal: {
    bar: { active: "Goal", paused: "Paused", blocked: "Blocked", complete: "Done" },
    states: { active: "Under way", paused: "Paused", blocked: "Blocked", complete: "Done" },
    paused: (by, until) =>
      by === "quota"
        ? until
          ? `Out of quota, goes on at ${until}`
          : "Out of quota"
        : by === "world"
          ? "Stopped, for you to decide"
          : by === "agent"
            ? "Paused at your word"
            : "Paused by you",
    pause: "Pause",
    resume: "Resume",
    clear: "Clear",
    expand: "Expand",
    cleared: "Goal cleared",
    undo: "Undo",
    failed: "Couldn't change the goal",
    changed: "The goal was just changed elsewhere; look before you change it",
    ask: "What should it get done",
    start: "Set as goal",
    save: "Save",
    done: "How it was done",
    blocked: "What blocks it",
    sentinel: "Sentinel",
    sentinelAt: (at) => `Looked at ${at}`,
    looked: "Looked at",
    saw: "Saw",
    thinks: "Thinks",
    worked: (ms) => {
      const minutes = Math.floor(ms / 60_000);
      if (minutes < 1) return "under a minute";
      if (minutes < 60) return `${minutes} min`;
      const hours = Math.floor(minutes / 60);
      if (hours < 48) return minutes % 60 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
      return hours % 24 ? `${Math.floor(hours / 24)} d ${hours % 24} h` : `${Math.floor(hours / 24)} d`;
    },
    at: (ms, now) => clock(ms, now, "en-GB", "", "tomorrow"),
  },
  collab: {
    empty: "No colleagues or subagents yet",
    here: "This conversation",
    states: { working: "Working", done: "Handed back", failed: "Failed", stopped: "Stopped", unknown: "Not known whether it still works" },
    goal: { active: "Under way", paused: "Paused", blocked: "Blocked", complete: "Done" },
    steps: (n) => `${n} step${n === 1 ? "" : "s"}`,
    separator: ", ",
    readOnly: "Read only",
    back: "Team",
    jump: "Go to it",
  },
};
