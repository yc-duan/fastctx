// The words of a conversation's plan panel.

import type { BlockWords } from "./blocks";

export interface PlanWords extends BlockWords {
  empty: string;
  edit: string;
  save: string;
  cancel: string;
  version: (version: number) => string;
}

export const zhPlan: PlanWords = {
  empty: "还没有计划",
  edit: "编辑",
  save: "保存",
  cancel: "取消",
  version: (version) => `第 ${version} 版`,
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
  decide: "For you to decide",
  suggested: "Suggested",
  answered: (option) => `Chose: ${option}`,
};
