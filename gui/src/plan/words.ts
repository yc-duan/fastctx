// The words of a conversation's plan panel.

import type { BlockWords } from "./blocks";

export interface PlanWords extends BlockWords {
  empty: string;
  edit: string;
  save: string;
  cancel: string;
  version: (version: number) => string;
  /** Shown when a save met a newer version written elsewhere. */
  conflict: string;
  /** Shown when a save did not go through. */
  unsaved: string;
  /** The user's words when they pick an option of a decision on the plan. */
  reply: (question: string, option: string) => string;
  /** How a thing a plan names stands now. */
  live: {
    working: (took: string) => string;
    resting: string;
    running: (took: string) => string;
    failed: string;
    /** How a sentinel stands, as its home answered. */
    sense: (phase: "starting" | "watching" | "satisfied" | "unable" | "withdrawn" | "unavailable") => string;
  };
}

export const zhPlan: PlanWords = {
  empty: "还没有计划",
  edit: "编辑",
  save: "保存",
  cancel: "取消",
  version: (version) => `第 ${version} 版`,
  conflict: "这份计划在别处改过了，再保存一次就以你的为准",
  unsaved: "没存上，再保存一次",
  reply: (question, option) => `${question}：${option}`,
  live: {
    working: (took) => `在干活，${took}`,
    resting: "歇着",
    running: (took) => `在跑，${took}`,
    failed: "出了问题",
    sense: (phase) => ({ starting: "刚开始", watching: "盯着", satisfied: "已成立", unable: "盯不成", withdrawn: "已撤掉", unavailable: "暂时问不到" })[phase],
  },
  decide: "要你定",
  suggested: "建议",
  answered: (option) => `已选：${option}`,
};

export const enPlan: PlanWords = {
  empty: "No plan yet",
  edit: "Edit",
  save: "Save",
  cancel: "Cancel",
  version: (version) => `Version ${version}`,
  conflict: "This plan was changed elsewhere; save again to keep yours",
  unsaved: "Not saved; save again",
  reply: (question, option) => `${question}: ${option}`,
  live: {
    working: (took) => `working, ${took}`,
    resting: "resting",
    running: (took) => `running, ${took}`,
    failed: "in trouble",
    sense: (phase) => ({ starting: "starting", watching: "watching", satisfied: "found it", unable: "can't watch", withdrawn: "withdrawn", unavailable: "can't be asked now" })[phase],
  },
  decide: "For you to decide",
  suggested: "Suggested",
  answered: (option) => `Chose: ${option}`,
};
