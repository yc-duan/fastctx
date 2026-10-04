// A conversation as Markdown, to keep or hand on: who said what in the
// order it was said, what the agent did as one line each, its plans and
// steps. Thinking is left out; tool output is left to the conversation.

import type { Words } from "../words";
import type { Conversation } from "./model";
import { headline } from "./tool";

export interface ExportNames {
  title: string;
  /** "Codex", the agent by its name. */
  agent: string;
  you: string;
  plan: string;
  waiting: string;
}

export function markdownOf(conversation: Conversation, words: Words, names: ExportNames): string {
  const out: string[] = [`# ${names.title}`];
  let speaker = "";
  let tools: string[] = [];
  const flush = () => {
    if (tools.length) out.push(tools.join("\n"));
    tools = [];
  };
  const turnTo = (who: string) => {
    flush();
    if (who !== speaker) out.push(`## ${who}`);
    speaker = who;
  };
  for (const entry of conversation.entries) {
    switch (entry.kind) {
      case "heard":
        turnTo(
          !entry.from || entry.from.kind === "user"
            ? names.you
            : entry.from.kind === "world"
              ? words.world
              : `${entry.from.address.resident} · ${entry.from.address.machine}`,
        );
        out.push(entry.text);
        break;
      case "prose":
        if (entry.prose === "thinking" || !entry.text.trim()) break;
        turnTo(names.agent);
        out.push(entry.prose === "plan" ? `**${names.plan}**\n\n${entry.text}` : entry.text);
        break;
      case "tool": {
        if (speaker !== names.agent) turnTo(names.agent);
        const line = headline(entry.tool, entry.ended, words);
        tools.push(`- ${line.action}${line.object ? ` \`${line.object.replace(/`/g, "'")}\`` : ""}`);
        break;
      }
      case "steps":
        turnTo(names.agent);
        out.push(entry.steps.map((step) => `- [${step.status === "done" ? "x" : " "}] ${step.text}`).join("\n"));
        break;
      case "ask":
        turnTo(names.agent);
        out.push(`> ${names.waiting} · ${entry.request}`);
        break;
      default:
        break;
    }
  }
  flush();
  return `${out.join("\n\n")}\n`;
}
