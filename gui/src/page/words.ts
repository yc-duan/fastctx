// What the page's own elements say, in the interface's language: the host
// tells the page its locale, and these follow it.

import { world } from "./element";
import type { AgentState } from "./tools";

const zh = {
  // A conversation is never asleep to the person reading it: one the World
  // let rest is idle (walkthrough item 20).
  state: { working: "工作中", waiting: "等待回复", idle: "空闲", asleep: "空闲", offline: "离线" } as Record<AgentState, string>,
  online: "在线",
  offline: "离线",
  device: "设备",
  agents: (n: number) => (n ? `${n} 位智能体` : "没有智能体"),
  more: "更早的",
  notFound: (what: string) => `未找到 ${what}`,
  notText: "不是文本",
  run: "运行",
  running: "运行中…",
  exit: (code: number | null) => (code === null ? "未完成" : `退出码 ${code}`),
  say: "发送",
  saying: "发送中…",
  delivered: "已送达",
  notDone: "失败",
  signal: (n: number) => `被信号 ${n} 终止`,
  jobState: { running: "运行中", exited: "已结束", stopped: "已停止", interrupted: "已中断", failed: "失败" } as Record<string, string>,
};

const en: typeof zh = {
  state: { working: "working", waiting: "waiting for your reply", idle: "idle", asleep: "idle", offline: "offline" },
  online: "online",
  offline: "offline",
  device: "device",
  agents: (n) => (n ? `${n} agent${n === 1 ? "" : "s"}` : "no agents"),
  more: "Earlier",
  notFound: (what) => `${what} is not here`,
  notText: "Not text",
  run: "Run",
  running: "Running…",
  exit: (code) => (code === null ? "did not finish" : `exit ${code}`),
  say: "Say",
  saying: "Sending…",
  delivered: "Delivered",
  notDone: "Failed",
  signal: (n) => `ended by signal ${n}`,
  jobState: { running: "running", exited: "finished", stopped: "stopped", interrupted: "interrupted", failed: "failed" },
};

export function words(): typeof zh {
  return world().context.locale.startsWith("en") ? en : zh;
}
