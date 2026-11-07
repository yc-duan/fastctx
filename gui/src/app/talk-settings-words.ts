// The words of the conversations' settings (`./talk-settings`).

import type { Host, SettingsTab } from "./talk-settings";

export interface TalkSettingsWords {
  title: string;
  tabs: Record<SettingsTab, string>;
  /** The agents, as the models tab's cards name them. */
  agents: Record<Host, string>;
}

export const zhTalkSettings: TalkSettingsWords = {
  title: "对话设置",
  tabs: { skills: "技能", mcp: "MCP", models: "模型", history: "历史", collab: "模型协作" },
  agents: { claude: "Claude Code", codex: "Codex" },
};

export const enTalkSettings: TalkSettingsWords = {
  title: "Conversation settings",
  tabs: { skills: "Skills", mcp: "MCP", models: "Models", history: "History", collab: "Collaboration" },
  agents: { claude: "Claude Code", codex: "Codex" },
};
