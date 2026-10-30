// The words of the panel beside a conversation, the goal's bar above the
// input, and the pages the panel holds.

import type { GoalStatus, NodeState, PausedBy } from "./types";

export interface SideWords {
  /** The conversation's own parts, each a word in the side's first line. */
  pages: { plan: string; goal: string; collab: string };
  /** The switch that opens the side, and the act that shuts it. */
  panel: string;
  shut: string;
  /** The line between the side and the conversation, dragged. */
  resize: string;
  close: string;
  /** A tab's conversation opened as the page's own. */
  openMain: string;
  /** Said where a conversation dragged from the list would open. */
  dropHere: string;
  /** The side's "⋯", and each place it may stand at, in it. */
  more: string;
  docks: { right: string; bottom: string; left: string };
  /** How another conversation comes into the side, said until one has. */
  dropHint: string;
  /** How far along the plan is, on its tab. */
  planCount: (reached: number, stops: number) => string;
  /** A side conversation's name where it has none yet. */
  sideTalk: string;
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
  /** A goal that is never done, as it is set and as it stands; the bar's
   *  name for it, and the one sentence saying what it is. */
  standing: string;
  barStanding: string;
  standingTip: string;
  /** Why a standing goal was paused: twice it stopped with nothing left to
   *  wake it. */
  unwokenWhy: string;
  /** The goals the conversation had before, how each ended, and when. */
  earlier: string;
  outcome: { done: string; dropped: string };
  span: (from: string, to: string) => string;
  separator: string;
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
  /** To the row of the call that sent one of the host's agents. */
  jump: string;
  /** How many work now, said on the team's tab. */
  atWork: (n: number) => string;
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
  shut: "收起侧面板",
  resize: "调整侧面板宽度",
  close: "关闭",
  openMain: "在主区打开",
  dropHere: "分屏",
  more: "更多",
  docks: { right: "停在右边", bottom: "停在下边", left: "停在左边" },
  dropHint: "把左边的一段对话拖过来，分屏来看",
  planCount: (reached, stops) => `${reached}/${stops}`,
  sideTalk: "侧边对话",
  goal: {
    bar: { active: "进行中的目标", paused: "已暂停", blocked: "阻碍", complete: "已完成" },
    states: { active: "进行中", paused: "已暂停", blocked: "阻碍", complete: "已完成" },
    paused: (by, until) =>
      by === "quota"
        ? until
          ? `额度用完，${until} 接着`
          : "额度用完"
        : by === "unwoken"
          ? "没留下叫醒它的"
          : by === "world"
            ? "停下了，等你定"
            : by === "agent"
              ? "照你的话停下"
              : "你停下的",
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
    standing: "一直挂着",
    barStanding: "一直挂着的目标",
    standingTip: "不以办成为止：每停一次，留下能叫醒它的，接着做下去",
    unwokenWhy: "连着两次停下，都没留下能叫醒它的东西",
    earlier: "以前的目标",
    outcome: { done: "办成了", dropped: "放下了" },
    span: (from, to) => `${from} 到 ${to}`,
    separator: "，",
    done: "完成说明",
    blocked: "阻碍说明",
    sentinel: "观测",
    sentinelAt: (at) => `${at} 查看`,
    looked: "查看",
    saw: "发现",
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
    states: { working: "在干", done: "交回了", failed: "失败", stopped: "已停下", idle: "空着", unknown: "不知道是否还在干" },
    goal: { active: "进行中", paused: "已暂停", blocked: "阻碍", complete: "已完成" },
    steps: (n) => `${n} 步`,
    separator: "，",
    jump: "转到它",
    atWork: (n) => `${n} 个在干活`,
  },
};

export const enSide: SideWords = {
  pages: { plan: "Plan", goal: "Goal", collab: "Team" },
  panel: "Side panel",
  shut: "Close the side panel",
  resize: "Resize the side panel",
  close: "Close",
  openMain: "Open as the main conversation",
  dropHere: "Split view",
  more: "More",
  docks: { right: "On the right", bottom: "At the foot", left: "On the left" },
  dropHint: "Drag a conversation here from the list to see both",
  planCount: (reached, stops) => `${reached}/${stops}`,
  sideTalk: "Side conversation",
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
    standing: "Standing",
    barStanding: "Standing goal",
    standingTip: "Never done: each time it stops it leaves something to wake it, and goes on",
    unwokenWhy: "It stopped twice in a row with nothing left to wake it",
    earlier: "Earlier goals",
    outcome: { done: "Done", dropped: "Put down" },
    span: (from, to) => `${from} to ${to}`,
    separator: ", ",
    done: "How it was done",
    blocked: "What blocks it",
    sentinel: "Watcher",
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
    states: { working: "Working", done: "Handed back", failed: "Failed", stopped: "Stopped", idle: "Idle", unknown: "Not known whether it still works" },
    goal: { active: "Under way", paused: "Paused", blocked: "Blocked", complete: "Done" },
    steps: (n) => `${n} step${n === 1 ? "" : "s"}`,
    separator: ", ",
    jump: "Go to it",
    atWork: (n) => `${n} at work`,
  },
};
