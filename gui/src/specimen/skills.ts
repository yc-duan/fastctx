// The specimen's account skills, kept for the page's life, with the hub's
// own rule on names.

import type { Skill, SkillCalls } from "../world/skills";

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export function specimenSkills(): SkillCalls {
  const kept: Skill[] = [
    {
      name: "release-notes",
      description: "写发版说明、更新日志时：先列用户看得见的变化，再列修好的问题",
      instructions: "## 发版说明\n\n1. 读这一版与上一版之间的提交。\n2. 先写用户看得见的变化，一条一句。\n3. 再写修好的问题，写清以前怎样、现在怎样。\n4. 不写内部重构。",
    },
    {
      name: "paper-review",
      description: "审一篇论文或技术报告时用：按贡献、证据、复现三件事给意见",
      instructions: "## 审稿\n\n- 贡献：它声称做成了什么，和已有的比新在哪里。\n- 证据：每个结论由哪张图、哪组实验支撑。\n- 复现：数据、代码、超参数是否给全。",
    },
    {
      name: "lab-safety",
      description: "动机房的机器之前：先看有没有人在用，晚上十点后不重启",
      instructions: "先 `w` 看谁在线、跑着什么作业；有人在用就别动。晚上十点之后不重启任何一台。",
    },
  ];
  return {
    list: async () => {
      await pause(120);
      return kept.map(({ name, description }) => ({ name, description }));
    },
    read: async (_hub, _account, name) => {
      await pause(80);
      return kept.find((one) => one.name === name) ?? null;
    },
    save: async (_hub, _account, previous, skill) => {
      await pause(300);
      if (kept.some((one) => one.name === skill.name && one.name !== previous)) throw new Error(`there is a skill named ${skill.name} already`);
      const at = kept.findIndex((one) => one.name === previous);
      if (at >= 0) kept[at] = skill;
      else kept.push(skill);
    },
    remove: async (_hub, _account, name) => {
      await pause(200);
      const at = kept.findIndex((one) => one.name === name);
      if (at >= 0) kept.splice(at, 1);
    },
  };
}
