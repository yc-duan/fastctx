// The specimen's account subagents: Fastctx's defaults until one is changed,
// kept for the page's life, with the hub's own rules on names and versions.

import type { SubagentOperation, SubagentSpec, SubagentView, Subagents } from "../world/subagents";

const RULES = "你是主代理派出的探子，只做探索、检索、核验：不改动任何东西，不做方案取舍或最终判断——那些是主代理的事。";

function defaults(): Subagents {
  const quiet = { model: "claude-haiku-5-5", effort: "medium", disallowedTools: ["Agent", "Artifact", "ExitPlanMode", "Edit", "Write", "NotebookEdit"] };
  return {
    agents: [
      { name: "Explore", description: "主代理的默认探子——宽而重的只读探索、定位、检索。用于跨文件/跨目录的搜索、命名约定排查、“这东西在哪 / 有哪些 / 长什么样”这类问题。", prompt: RULES, ...quiet, color: "cyan" },
      { name: "Plan", description: "软件架构探子——为某个任务设计实现方案。需要“该怎么动手”的实现策略、关键文件清单、架构取舍时用它。", prompt: RULES, ...quiet, color: "purple" },
      { name: "general-purpose", description: "较难的、需要多步或把握不准的只读探索与检索。当宽扇出的 Explore 不足以判断、需要顺藤摸瓜地多轮定位时用它。", prompt: RULES, ...quiet, color: "green" },
      ...Array.from({ length: Number(new URLSearchParams(location.search).get("subagents") ?? 0) }, (_, at) => ({
        name: `reader-${at + 1}`,
        description: "读一份长文档，交回其中与问题有关的段落。",
        prompt: RULES,
        ...quiet,
      })),
    ],
    instructions:
      "## Claude 子代理\n\n你需要善用勤用子代理，最多同时并行 8 个。何时派发、如何分工由主代理决定，并由主代理负责最终整合与验证。有独立、明确的问题需要探查、检索或审视时再委派，不为增加复核次数而重复派发已经有结论的工作。\n\n子代理默认只读探查、检索和整理证据，也可提供独立观点、承担机械整理工作，减少探索噪音占用主代理上下文。只交回精炼结论、必要证据及不确定性。设计与实现由主代理负责，这部分不外派。\n\n- 子代理不得再派生子代理；委派时在任务说明中写明。\n",
  };
}

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export function specimenSubagents(older: boolean): (hub: string, account: string, operation: SubagentOperation) => Promise<SubagentView> {
  let version = 6;
  let own: Subagents | null = null;
  return async (hub, account, operation) => {
    await pause(operation.operation === "list" ? 200 : 350);
    if (operation.operation !== "list") {
      if (operation.version !== version) throw new Error(`the profile changed: version ${operation.version} was read, current version is ${version}`);
      const next: Subagents = structuredClone(own ?? defaults());
      switch (operation.operation) {
        case "save": {
          const at = operation.previous === null ? -1 : next.agents.findIndex((agent) => agent.name === operation.previous);
          if (next.agents.some((agent, index) => index !== at && agent.name === operation.agent.name)) throw new Error(`subagent ${operation.agent.name} already exists`);
          if (at >= 0) next.agents[at] = operation.agent as SubagentSpec;
          else next.agents.push(operation.agent as SubagentSpec);
          break;
        }
        case "delete":
          next.agents = next.agents.filter((agent) => agent.name !== operation.name);
          break;
        case "instructions":
          next.instructions = operation.text;
          break;
        case "reset":
          break;
      }
      own = operation.operation === "reset" ? null : next;
      version += 1;
    }
    return { hub, account, supported: !older, version, configured: own !== null, configuration: own ?? defaults() };
  };
}
