// What the mode page says (`world-layers.md`): the two ways to use Fastctx,
// asked once at first and changed here after.

export interface ModeWords {
  title: string;
  /** Asked when the account has not chosen yet. */
  ask: string;
  connect: { name: string; line: string };
  workbench: { name: string; line: string };
  using: string;
  choose: string;
  switchTo: string;
  /** Going back to Connect, asked once. */
  toConnect: { title: string; what: string; go: string };
  failed: string;
}

export const zhMode: ModeWords = {
  title: "模式",
  ask: "你想怎样用 Fastctx？",
  connect: { name: "接入", line: "在你自己的 Claude Code、Codex 里干活，它们多了几台机器的手；干了什么，在「活动」里看。" },
  workbench: { name: "工作台", line: "在 Fastctx 里和智能体对话，它替你一直干下去；接入的一切也都在。" },
  using: "正在用",
  choose: "用这个",
  switchTo: "改用",
  toConnect: { title: "改回接入", what: "Fastctx 的对话停在原处：不再开新的一轮，给它们的话存着，改回工作台时接着。", go: "改回接入" },
  failed: "没改成",
};

export const enMode: ModeWords = {
  title: "Mode",
  ask: "How do you want to use Fastctx?",
  connect: { name: "Connect", line: "Work in your own Claude Code and Codex, with your other machines at hand; see what they did in Activity." },
  workbench: { name: "Workbench", line: "Talk to agents in Fastctx and let them carry the work on; everything in Connect comes with it." },
  using: "In use",
  choose: "Use this",
  switchTo: "Switch",
  toConnect: { title: "Back to Connect", what: "Fastctx's conversations stay where they are: no new turns start, what comes for them is kept, and they carry on when the workbench is back.", go: "Back to Connect" },
  failed: "Not changed",
};
