// Markdown sent by a colleague or returned by a host's agent, read in a
// bounded region with the same wheel handoff as tool output.

import { handOnAtEdge } from "../scroll-handoff";
import { render } from "./prose";

export function noticeAnswer(text: string): HTMLElement {
  const answer = document.createElement("div");
  answer.className = "prose notice-answer";
  answer.innerHTML = render(text);
  handOnAtEdge(answer);
  return answer;
}
