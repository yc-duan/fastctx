// The interface's entry. Until it is joined to a world, it shows the
// specimens: the front page at `#/`, a conversation at `#/talk/<id>@<machine>`.
// `?s=home|claude|codex` opens one directly, `?theme=…` and `?lang=en` set
// the theme and the words, and `?live` plays a conversation in as it would
// arrive.

import "katex/dist/katex.min.css";
import "./design/fonts.css";
import "./design/tokens.css";
import "./design/base.css";
import "./design/highlight.css";
import "./conversation/conversation.css";
import "./app.css";
import "./home/home.css";

import { Conversation } from "./conversation/model";
import { ConversationPage } from "./conversation/page";
import { redrawDiagrams } from "./conversation/diagrams";
import { HomePage } from "./home/page";
import { enHome, zhHome } from "./home/words";
import { specimens } from "./specimen/sessions";
import { specimenWorld } from "./specimen/world";
import { applyTheme } from "./theme";
import { en, zh } from "./words";

const params = new URLSearchParams(location.search);
const english = params.get("lang") === "en";
const words = english ? en : zh;
const homeWords = english ? enHome : zhHome;
document.documentElement.lang = english ? "en" : "zh-CN";

const app = document.getElementById("app")!;
let current: HTMLElement | null = null;

function show(page: HTMLElement, title: string): void {
  current?.remove();
  current = page;
  app.append(page);
  document.title = title;
  scrollTo({ top: 0 });
}

function home(): void {
  const { world, now, lastVisit } = specimenWorld();
  const page = new HomePage({
    words: homeWords,
    ended: words.ended,
    world,
    now,
    lastVisit,
    link: (address) => `#/talk/${encodeURIComponent(`${address.resident}@${address.machine}`)}`,
  });
  show(page.element, homeWords.world);
}

function talk(name: string, back: boolean): void {
  // Each resident opens the specimen conversation of its kind.
  const specimen = (specimens[name.startsWith("codex") ? "codex" : "claude"] ?? specimens.claude)();
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
    back: back ? { href: "#/", label: homeWords.world } : null,
    conversation,
    send(text) {
      conversation.sending(text, specimen.here);
      page.update();
    },
    stop() {},
  });
  show(page.element, `${specimen.title} · World`);
  if (params.has("live")) {
    let index = 0;
    const next = () => {
      const event = specimen.events[index++];
      if (!event || current !== page.element) return;
      conversation.event(event);
      page.update();
      setTimeout(next, event.event === "heard" ? 900 : 420);
    };
    next();
  } else {
    conversation.history(specimen.events);
    page.update();
  }
}

function route(): void {
  const direct = params.get("s");
  if (direct && !location.hash) {
    if (direct === "home") home();
    else talk(direct, false);
    return;
  }
  const talking = /^#\/talk\/(.+)$/.exec(location.hash);
  if (talking) talk(decodeURIComponent(talking[1]), true);
  else home();
}

addEventListener("hashchange", route);
applyTheme(() => {
  if (current) void redrawDiagrams(current);
});
route();
