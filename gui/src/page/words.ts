// What the page's own elements say, in the interface's language: the host
// tells the page its locale, and these follow it.

import { world } from "./element";
import type { AgentState } from "./tools";

const zh = {
  state: { working: "在干活", waiting: "在等你回答", idle: "空闲", asleep: "睡着了", offline: "离线" } as Record<AgentState, string>,
  online: "在线",
  offline: "离线",
  device: "设备",
  agents: (n: number) => (n ? `${n} 位智能体` : "没有智能体"),
  noEntries: "还没有条目",
  more: "更早的",
  notFound: (what: string) => `世界里没有 ${what}`,
  notText: "不是文本",
  run: "跑",
  running: "正在跑…",
  exit: (code: number | null) => (code === null ? "没跑完" : `退出码 ${code}`),
  say: "说",
  saying: "正在送…",
  delivered: "送到了",
  jobState: { running: "在跑", exited: "跑完了", interrupted: "中断了" } as Record<string, string>,
};

const en: typeof zh = {
  state: { working: "working", waiting: "waiting for you", idle: "idle", asleep: "asleep", offline: "offline" },
  online: "online",
  offline: "offline",
  device: "device",
  agents: (n) => (n ? `${n} agent${n === 1 ? "" : "s"}` : "no agents"),
  noEntries: "No entries yet",
  more: "Earlier",
  notFound: (what) => `${what} is not in the World`,
  notText: "Not text",
  run: "Run",
  running: "Running…",
  exit: (code) => (code === null ? "did not finish" : `exit ${code}`),
  say: "Say",
  saying: "Sending…",
  delivered: "Delivered",
  jobState: { running: "running", exited: "finished", interrupted: "interrupted" },
};

export function words(): typeof zh {
  return world().context.locale.startsWith("en") ? en : zh;
}
