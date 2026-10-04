// The interface's entry: the front page at `#/`, a conversation at
// `#/talk/<resident>@<machine>`. Inside the shell they read the World on
// this machine; in a plain browser they show the specimens, and
// `?s=home|claude|codex`, `?theme=…`, `?lang=en` and `?live` pick what.

import "katex/dist/katex.min.css";
import "./design/fonts.css";
import "./design/tokens.css";
import "./design/base.css";
import "./design/highlight.css";
import "./conversation/conversation.css";
import "./app.css";
import "./home/home.css";

import { Conversation } from "./conversation/model";
import { ConversationPage, type Someone } from "./conversation/page";
import { redrawDiagrams } from "./conversation/diagrams";
import { HomePage } from "./home/page";
import { enHome, zhHome } from "./home/words";
import { specimens } from "./specimen/sessions";
import { specimenWorld } from "./specimen/world";
import { applyTheme } from "./theme";
import type { Address } from "./world/events";
import { inShell, open as openAgent, stop, talk, unwatch, watch, worldNow, type Piece, type WorldNow } from "./world/bridge";
import type { WorldState } from "./home/model";
import { en, zh } from "./words";

const params = new URLSearchParams(location.search);
const english = params.get("lang") === "en";
const words = english ? en : zh;
const homeWords = english ? enHome : zhHome;
document.documentElement.lang = english ? "en" : "zh-CN";

const live = inShell();
const app = document.getElementById("app")!;
let current: HTMLElement | null = null;
/** Undoes what the page now shown keeps running: a refresh, a watch. */
let leave: (() => void) | null = null;
/** The World as last read, for a conversation opened from the front page. */
let latest: WorldNow | null = null;

function show(page: HTMLElement, title: string, onLeave: (() => void) | null = null): void {
  leave?.();
  leave = onLeave;
  current?.remove();
  current = page;
  app.append(page);
  document.title = title;
  scrollTo({ top: 0 });
}

// When the reader was last here: kept on this device, read once as the
// interface opens, then kept up to date while it is open.
const lastSeen = (() => {
  try {
    const stored = localStorage.getItem("lastSeen");
    return stored ? new Date(Number(stored)) : null;
  } catch {
    return null;
  }
})();

function seenNow(): void {
  try {
    localStorage.setItem("lastSeen", String(Date.now()));
  } catch {
    // A device that keeps nothing simply never says when the reader was last here.
  }
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);
const linkTo = (address: Address) => `#/talk/${encodeURIComponent(`${address.resident}@${address.machine}`)}`;

function stateOf(now: WorldNow): WorldState {
  return { here: now.here, machines: now.who.machines, nodes: now.nodes.machines, towns: now.towns, happenings: null };
}

function home(): void {
  if (!live) {
    const { world, now, lastVisit } = specimenWorld();
    const page = new HomePage({ words: homeWords, ended: words.ended, lastVisit, link: linkTo, open: async () => {} });
    show(page.element, homeWords.world);
    page.update(world, now);
    return;
  }
  const page = new HomePage({
    words: homeWords,
    ended: words.ended,
    lastVisit: lastSeen,
    link: linkTo,
    // The agent asked in is opened at once: its first turn is watched from
    // the start, as any conversation is.
    open: async ({ agent, machine, town, text }) => {
      const { to } = await openAgent(agent, machine, town, text);
      location.hash = linkTo(to);
    },
  });
  let timer = 0;
  let left = false;
  // Asked again every two seconds while the page is in view, as the
  // terminal's town page does; a page out of view asks nothing.
  const refresh = async () => {
    if (left) return;
    if (document.visibilityState === "visible") {
      try {
        latest = await worldNow();
        if (!left) page.update(stateOf(latest), new Date());
        seenNow();
      } catch (error) {
        if (!left) page.unreachable(String(error), new Date());
      }
    }
    if (!left) timer = window.setTimeout(refresh, 2000);
  };
  show(page.element, homeWords.world, () => {
    left = true;
    clearTimeout(timer);
  });
  void refresh();
}

function specimenTalk(name: string, back: boolean): void {
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
    link: linkTo,
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

async function liveTalk(address: Address): Promise<void> {
  let world = latest;
  if (!world) {
    try {
      world = latest = await worldNow();
    } catch {
      world = null;
    }
  }
  const known = world?.who.machines
    .flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : []))
    .find((r) => r.about.id === address.resident && r.about.machine === address.machine);
  const someone: Someone = {
    agent: known ? agentName(known.about.host.agent) : address.resident,
    machine: address.machine,
    model: known?.about.model ?? null,
    town: known?.town ?? null,
  };
  const here = world?.here ?? null;
  const conversation = new Conversation();
  const page: ConversationPage = new ConversationPage({
    words,
    here: here ?? "",
    someone,
    back: { href: "#/", label: homeWords.world },
    link: linkTo,
    conversation,
    send(text) {
      const key = conversation.sending(text, here);
      page.update();
      talk(address, text)
        .then(({ to, landed }) => {
          // Words in a turn take their place when the session hears them;
          // words in no turn yet say where they are.
          if (landed.kind === "waiting" || landed.kind === "sent" || landed.kind === "uncertain") {
            const why = "why" in landed ? landed.why : null;
            conversation.sent(key, { state: "landed", how: landed.kind, machine: to.machine, why });
            page.update();
          }
        })
        .catch((error: unknown) => {
          conversation.sent(key, { state: "failed", why: String(error) });
          page.update();
        });
    },
    stop() {
      void stop(address);
    },
  });
  let watchId: number | null = null;
  let left = false;
  let timer = 0;
  show(page.element, `${someone.agent} · ${someone.machine}`, () => {
    left = true;
    clearTimeout(timer);
    if (watchId !== null) void unwatch(watchId);
  });
  // What it started that still runs, asked as the front page asks.
  const jobs = async () => {
    if (left) return;
    if (document.visibilityState === "visible") {
      try {
        latest = await worldNow();
        const running = latest.who.machines
          .flatMap((machine) => (machine.kind === "answered" ? machine.reply.jobs : []))
          .filter((job) => job.owner.resident === address.resident && job.owner.machine === address.machine);
        if (!left) page.running(running, new Date());
      } catch {
        // The watch says whether the World is reachable; the list keeps
        // what it last showed until the next answer.
      }
    }
    if (!left) timer = window.setTimeout(jobs, 2000);
  };
  void jobs();
  // Events can come before the history does; they wait for it.
  let ready = false;
  const early: Piece[] = [];
  const take = (piece: Piece) => {
    if (piece.kind === "event") conversation.event(piece.event);
    else page.lost(piece.error);
  };
  try {
    const started = await watch(address, (piece) => {
      if (left) return;
      if (!ready) early.push(piece);
      else {
        take(piece);
        page.update();
      }
    });
    watchId = started.id;
    if (left) {
      void unwatch(started.id);
      return;
    }
    conversation.history(started.history);
    for (const piece of early) take(piece);
    ready = true;
    page.update();
  } catch (error) {
    page.lost(String(error));
    page.update();
  }
}

function route(): void {
  const direct = params.get("s");
  if (!live && direct && !location.hash) {
    if (direct === "home") home();
    else specimenTalk(direct, false);
    return;
  }
  const talking = /^#\/talk\/(.+)$/.exec(location.hash);
  if (!talking) {
    home();
    return;
  }
  const name = decodeURIComponent(talking[1]);
  const at = name.lastIndexOf("@");
  if (live && at > 0) void liveTalk({ resident: name.slice(0, at), machine: name.slice(at + 1) });
  else specimenTalk(name, true);
}

addEventListener("hashchange", route);
applyTheme(() => {
  if (current) void redrawDiagrams(current);
});
route();
