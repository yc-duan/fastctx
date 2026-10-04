// The interface's entry. Until it is joined to a world, it opens one of the
// specimen sessions: `?s=claude|codex`, `?theme=…`, `?lang=en`, and
// `?live` to watch the session arrive event by event.

import "katex/dist/katex.min.css";
import "./design/fonts.css";
import "./design/tokens.css";
import "./design/base.css";
import "./design/highlight.css";
import "./conversation/conversation.css";
import "./app.css";

import { Conversation } from "./conversation/model";
import { ConversationPage } from "./conversation/page";
import { redrawDiagrams } from "./conversation/diagrams";
import { specimens } from "./specimen/sessions";
import { applyTheme } from "./theme";
import { en, zh } from "./words";

const params = new URLSearchParams(location.search);
const words = params.get("lang") === "en" ? en : zh;
document.documentElement.lang = words === en ? "en" : "zh-CN";

const specimen = (specimens[params.get("s") ?? "claude"] ?? specimens.claude)();
const conversation = new Conversation();
const page = new ConversationPage({
  words,
  here: specimen.here,
  someone: {
    agent: specimen.agent === "claude" ? "Claude" : "Codex",
    machine: specimen.machine,
    model: specimen.model,
    town: specimen.town,
  },
  conversation,
  send(text) {
    conversation.sending(text, specimen.here);
    page.update();
  },
  stop() {},
});
document.getElementById("app")!.append(page.element);
document.title = `${specimen.title} · World`;
applyTheme(() => void redrawDiagrams(page.element));

if (params.has("live")) {
  let index = 0;
  const next = () => {
    const event = specimen.events[index++];
    if (!event) return;
    conversation.event(event);
    page.update();
    setTimeout(next, event.event === "heard" ? 900 : 420);
  };
  next();
} else {
  conversation.history(specimen.events);
  page.update();
}
