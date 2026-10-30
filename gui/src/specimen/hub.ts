// The project page of mark50 with its work under way (`?s=project`): four
// conversations begun there, each with a plan's way, two colleagues, three
// senses on the machines they watch, one question for the user and two
// things told that the user has not seen.

import type { SentinelSummary } from "../world/sentinels";

/** Each conversation's plan, by `resident@machine`. */
export function specimenHubPlans(): Map<string, string | null> {
  return new Map<string, string | null>([
    [
      "claude-2b1c@laptop",
      "# PID 环改定点\n\n```flow\n读懂溢出 -> 积分放宽到 Q32 -> *整套回归 -> 合进主线 -> 真机验证\n```\n",
    ],
    [
      "claude-v01c@laptop",
      "# 夜里的回归与训练\n\n```flow\n整套回归 -> 训练 run3 -> *run3b 重跑 -> 定学习率\n```\n",
    ],
    [
      "claude-a7e2@laptop",
      "# 真机验证\n\n```flow\n列清单 -> *借真机 -> 跑 50 次 -> 写报告\n借真机 --> 台架代替: 借不到时\n```\n",
    ],
    ["codex-4c1e@desktop", "# CAN 帧时间戳\n\n```flow\n抓包 -> 驱动层打戳 -> 应用层去重 -> *回放验收\n```\n"],
  ]);
}

const owner = (resident: string, machine: string) => ({ machine, resident });

/** The senses the accounts here keep, three of them for mark50. */
export function specimenHubSenses(): SentinelSummary[] {
  return [
    {
      definition: { id: "s-7f2c", home: "gpu", condition: "run3b 的 loss 再次发散或停住", kind: "local", owner: owner("claude-v01c", "laptop"), generation: 1 },
      phase: "watching",
      plan: "每 5 分钟读一次 /data/run3b/train.log 的最后 200 行",
      latest: { at: Date.parse("2026-10-04T16:50:00Z"), text: "step 12,400 loss 0.291，平稳" },
      reason: null,
    },
    {
      definition: { id: "s-c14b", home: "lab", condition: "CI 跑完 ctrl 的整套测试", kind: "local", owner: owner("claude-2b1c", "laptop"), generation: 1 },
      phase: "watching",
      plan: "看 /srv/mark50-ci/runs 下最新一轮的结果文件",
      latest: null,
      reason: null,
    },
    {
      definition: { id: "s-n8d1", home: "desktop", condition: "CAN 总线上出现时间戳倒退的帧", kind: "local", owner: owner("codex-4c1e", "desktop"), generation: 1 },
      phase: "watching",
      plan: "跟着 candump 的输出比相邻两帧的时间戳",
      latest: { at: Date.parse("2026-10-04T15:20:00Z"), text: "这一小时没有倒退的帧" },
      reason: null,
    },
  ];
}
