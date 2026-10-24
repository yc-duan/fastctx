// What the page's own elements say, in the interface's language: the host
// tells the page its locale, and these follow it.

import { world } from "./element";
import type { AgentState } from "./tools";

const zh = {
  // A conversation is never asleep to the person reading it: one the World
  // let rest is idle (walkthrough item 20).
  state: { working: "在干活", waiting: "等你回复", idle: "空闲", asleep: "空闲", offline: "离线" } as Record<AgentState, string>,
  online: "在线",
  offline: "离线",
  device: "设备",
  agents: (n: number) => (n ? `${n} 位智能体` : "没有智能体"),
  more: "更早的",
  notFound: (what: string) => `这里没有 ${what}`,
  notText: "不是文本",
  run: "跑",
  running: "正在跑…",
  exit: (code: number | null) => (code === null ? "没跑完" : `退出码 ${code}`),
  say: "说",
  saying: "正在送…",
  delivered: "送到了",
  signal: (n: number) => `被信号 ${n} 结束`,
  jobState: { running: "在跑", exited: "跑完了", stopped: "被停下了", interrupted: "中断了", failed: "出错了" } as Record<string, string>,
};

const en: typeof zh = {
  state: { working: "working", waiting: "waiting for your reply", idle: "idle", asleep: "idle", offline: "offline" },
  online: "online",
  offline: "offline",
  device: "device",
  agents: (n) => (n ? `${n} agent${n === 1 ? "" : "s"}` : "no agents"),
  more: "Earlier",
  notFound: (what) => `${what} isn't here`,
  notText: "Not text",
  run: "Run",
  running: "Running…",
  exit: (code) => (code === null ? "did not finish" : `exit ${code}`),
  say: "Say",
  saying: "Sending…",
  delivered: "Delivered",
  signal: (n) => `ended by signal ${n}`,
  jobState: { running: "running", exited: "finished", stopped: "stopped", interrupted: "interrupted", failed: "failed" },
};

export function words(): typeof zh {
  return world().context.locale.startsWith("en") ? en : zh;
}
