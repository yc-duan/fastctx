// The words of the panel beside a conversation, the goal's bar above the
// input, and the pages the panel holds.

import { en, zh } from "../words";
import type { GoalStatus, NodeState, PausedBy } from "./types";

export interface SideWords {
  /** The conversation's own parts, each a word in the side's first line. */
  pages: { plan: string; goal: string; collab: string; history: string };
  /** The side's first page: the parts at a glance. */
  home: { name: string; none: string; all: (n: number) => string };
  /** A file read in the side, over the parts. */
  file: FileWords;
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
  where: WhereWords;
  goal: GoalWords;
  collab: CollabWords;
}

/** Where the conversation works, in the desk's corner and pulled out. */
export interface WhereWords {
  /** Its name, said on hover and to a screen reader. */
  title: string;
  /** How many more places it works in than its line names. */
  more: (n: number) => string;
  /** A computer away now, said on hover alone: it is drawn in grey. */
  away: string;
  /** One of another account assisting the user: whose agent it is. */
  from: (who: string) => string;
  /** A folder on this computer, shown in the system's own window. */
  reveal: string;
}

export interface FileWords {
  /** Opens the file with this device's program for its kind. */
  outside: string;
  reading: string;
  /** The machine has no file there. */
  missing: string;
  /** It could not be read; the original goes on hover. */
  failed: string;
  /** The way to it broke; it comes on once the way is back. */
  interrupted: string;
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
  reading: string;
  states: Record<NodeState, string>;
  goal: Record<GoalStatus, string>;
  /** How long one worked, to the second while it works. */
  took: (ms: number) => string;
  /** The clock's two ends: how long ago it begins, and now. */
  ago: (span: string) => string;
  now: string;
  /** The page's groups: the colleagues it asked, each with the agents it
   *  sent, the host's own agents, and the conversations branched off it. */
  groups: { colleagues: string; agents: string; side: string };
  /** Over the agents a colleague sent, inside its card. */
  sent: (n: number) => string;
  /** Those that ended long before the rest, folded into one row. */
  earlier: (n: number) => string;
  fold: string;
  began: (at: string) => string;
  tokens: (n: number) => string;
  steps: (n: number) => string;
  /** Between the facts of one line said when pointed at. */
  separator: string;
  /** To the row of the call that sent one of the host's agents. */
  jump: string;
  /** From one of the host's agents back to who works with the conversation. */
  back: string;
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
  pages: { plan: "计划", goal: "目标", collab: "协作", history: "笔记" },
  home: { name: "概览", none: "还没有计划与协作", all: (n) => `全部 ${n} 个` },
  file: { outside: "用默认程序打开", reading: "读取中…", missing: "找不到这个文件", failed: "读取失败", interrupted: "连接中断，正在重连…" },
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
  where: { title: "所在的电脑与文件夹", more: (n) => `+${n}`, away: "离线", from: (who) => `来自 ${who}`, reveal: "在文件夹中显示" },
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
    reading: "还在读",
    states: { working: "在干", waiting: "等你回答", done: "交回了", failed: "失败", stopped: "已停下", idle: "空着", unknown: "不知道是否还在干" },
    goal: { active: "进行中", paused: "已暂停", blocked: "阻碍", complete: "已完成" },
    took: zh.took,
    ago: (span) => `${span}前`,
    now: "现在",
    groups: { colleagues: "模型协作", agents: "子代理", side: "侧边对话" },
    sent: (n) => `派出 ${n} 个子代理`,
    earlier: (n) => `更早 ${n} 个`,
    fold: "收起更早的",
    began: (at) => `${at} 开始`,
    tokens: (n) => (n >= 10_000 ? `${(n / 10_000).toFixed(1)} 万 token` : `${n} token`),
    steps: (n) => `${n} 步`,
    separator: "，",
    jump: "转到它",
    back: "返回协作",
    atWork: (n) => `${n} 个在干活`,
  },
};

export const enSide: SideWords = {
  pages: { plan: "Plan", goal: "Goal", collab: "Team", history: "Notes" },
  home: { name: "Overview", none: "No plan or helpers yet", all: (n) => `All ${n}` },
  file: { outside: "Open with default app", reading: "Reading…", missing: "File not found", failed: "Couldn't read this file", interrupted: "Disconnected, reconnecting…" },
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
  where: { title: "Computer and folders it works in", more: (n) => `+${n}`, away: "Offline", from: (who) => `From ${who}`, reveal: "Show in folder" },
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
    reading: "Still reading",
    states: { working: "Working", waiting: "Waiting for you", done: "Handed back", failed: "Failed", stopped: "Stopped", idle: "Idle", unknown: "Not known whether it still works" },
    goal: { active: "Under way", paused: "Paused", blocked: "Blocked", complete: "Done" },
    took: en.took,
    ago: (span) => `${span} ago`,
    now: "now",
    groups: { colleagues: "Model collaboration", agents: "Subagents", side: "Side conversations" },
    sent: (n) => `Sent ${n} subagent${n === 1 ? "" : "s"}`,
    earlier: (n) => `${n} earlier`,
    fold: "Fold the earlier ones",
    began: (at) => `Began ${at}`,
    tokens: (n) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k tokens` : `${n} tokens`),
    steps: (n) => `${n} step${n === 1 ? "" : "s"}`,
    separator: ", ",
    jump: "Go to it",
    back: "Back to the team",
    atWork: (n) => `${n} at work`,
  },
};
