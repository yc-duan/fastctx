// The interface's entry: the frame with its sidebar, and the place the
// address names in it (`app/places.ts`). Inside the shell it reads the
// World on this machine; in a plain browser a specimen stands in, and
// `?s=home|claude|codex|project|journal|machines|settings|new`,
// `?theme=…`, `?lang=en` and `?live` pick what to look at.

import "katex/dist/katex.min.css";
import "./design/fonts.css";
import "./design/tokens.css";
import "./design/base.css";
import "./design/highlight.css";
import "./conversation/conversation.css";
import "./app/shell.css";
import "./app/sheet.css";
import "./app/pages.css";
import "./app.css";
import "./home/home.css";
import "./project/project.css";

import { Devices } from "./app/devices";
import { MachinesPage } from "./app/machines";
import { NewTalk } from "./app/new-talk";
import { Palette, type PaletteItem } from "./app/palette";
import { ProjectForm } from "./app/project-form";
import { link, placeOf, talkLink, type Place } from "./app/places";
import { SettingsPage } from "./app/settings";
import { WelcomePage } from "./app/welcome";
import { Shell, pageHead, type Page } from "./app/shell";
import { effect } from "./app/signals";
import { createStore, jobsOf, residentsOf } from "./app/store";
import {
  enDetails,
  enDevices,
  enMachines,
  enNewTalk,
  enPalette,
  enProjectForm,
  enSettings,
  enShell,
  enWelcome,
  zhDetails,
  zhDevices,
  zhMachines,
  zhNewTalk,
  zhPalette,
  zhProjectForm,
  zhSettings,
  zhShell,
  zhWelcome,
} from "./app/words";
import { Details } from "./conversation/details";
import { Conversation } from "./conversation/model";
import { ConversationPage, type Someone } from "./conversation/page";
import { redrawDiagrams } from "./conversation/diagrams";
import { HomePage } from "./home/page";
import type { Happening, WorldState } from "./home/model";
import { enHome, zhHome } from "./home/words";
import { installPens } from "./pen/filters";
import { installRules } from "./pen/rules";
import { JournalPage } from "./project/journal";
import { JournalForm } from "./project/journal-form";
import { ProjectPage } from "./project/page";
import { enProject, zhProject } from "./project/words";
import { specimenJournal, specimenMap } from "./specimen/project";
import { specimens } from "./specimen/sessions";
import { specimenNow } from "./specimen/world";
import { applyTheme, chooseTheme, themes } from "./theme";
import type { Address } from "./world/events";
import type { ResidentDetail } from "./world/world";
import {
  applyHosts,
  hubHere,
  invite,
  joinWorld,
  placeFastctx,
  setupState,
  journal as readJournal,
  journalAdd,
  leave as leaveWorld,
  look,
  projectEdit,
  projectNew,
  open as openAgent,
  project as readProject,
  revoke,
  stop,
  talk,
  unwatch,
  watch,
  type Piece,
  type WorldNow,
} from "./world/bridge";
import { en, zh } from "./words";

const params = new URLSearchParams(location.search);
const language: "zh" | "en" = (() => {
  const asked = params.get("lang");
  if (asked === "en" || asked === "zh") return asked;
  try {
    return localStorage.getItem("lang") === "en" ? "en" : "zh";
  } catch {
    return "zh";
  }
})();
const english = language === "en";
const words = english ? en : zh;
const homeWords = english ? enHome : zhHome;
const projectWords = english ? enProject : zhProject;
const shellWords = english ? enShell : zhShell;
document.documentElement.lang = english ? "en" : "zh-CN";

const specimen = specimenNow();
const store = createStore(() => ({ world: specimen.world, now: specimen.now }));
const live = store.live;

const head = (title: string, actions?: string) => pageHead(shellWords, title, actions);

let place: Place = { kind: "home" };

const newTalk = new NewTalk({
  store,
  words: english ? enNewTalk : zhNewTalk,
  async begin({ agent, machine, town, text }) {
    if (!live) throw new Error(english ? "this is a specimen" : "这是样例，没有连着世界");
    const { to } = await openAgent(agent, machine, town, text);
    void store.refresh();
    return to;
  },
  opened(address) {
    location.hash = talkLink(address);
  },
});

const projectForm = new ProjectForm({
  store,
  words: english ? enProjectForm : zhProjectForm,
  create(town, fields) {
    if (!live) return Promise.reject(new Error((english ? "this is a specimen" : "这是样例，没有连着世界")));
    return projectNew(town, fields);
  },
  edit(town, fields) {
    if (!live) return Promise.reject(new Error((english ? "this is a specimen" : "这是样例，没有连着世界")));
    return projectEdit(town, fields);
  },
  done(town) {
    void store.refresh();
    const there = link({ kind: "project", town });
    if (location.hash === there) show(placeOf(there));
    else location.hash = there;
  },
});

const deviceWords = english ? enDevices : zhDevices;
const notLive = () => Promise.reject(new Error(deviceWords.specimen));
const devices = new Devices({
  words: deviceWords,
  // A specimen shows what a code looks like; it admits nobody.
  invite: (name, hours) =>
    live ? invite(name, hours) : Promise.resolve(`fxw3.${"specimen-not-a-real-invitation-".repeat(20)}${name ?? ""}${hours}`),
  revoke: (name, reason) => (live ? revoke(name, reason) : notLive()),
  leave: () => (live ? leaveWorld() : notLive()),
  when: (at) => when(at, new Date()),
  changed: () => void store.refresh(),
});

const journalForm = new JournalForm({
  words: projectWords,
  write: (town, journal, text, description) => (live ? journalAdd(town, journal, text, description) : notLive()),
  written(town, journal) {
    const there = link({ kind: "journal", town, journal });
    if (location.hash === there) show(placeOf(there));
    else location.hash = there;
  },
});

const shell = new Shell({
  store,
  words: shellWords,
  newTalk: () => newTalk.open(place.kind === "project" ? place.town : null),
  newProject: () => projectForm.open(),
});
document.getElementById("app")!.append(shell.element);

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

/** When the reader last looked at the front page: kept on this device, and
 *  brought up to date while the front page is in view, so what it says
 *  happened "while you were away" is what they have not seen there. */
const seen = {
  read(): Date | null {
    try {
      const stored = localStorage.getItem("lastSeen");
      return stored ? new Date(Number(stored)) : null;
    } catch {
      return null;
    }
  },
  now(): void {
    try {
      localStorage.setItem("lastSeen", String(Date.now()));
    } catch {
      // A device that keeps nothing never says when the reader was last here.
    }
  },
};

function stateOf(world: WorldNow, happenings: Happening[] | null): WorldState {
  return { here: world.here, machines: world.who.machines, nodes: world.nodes.machines, towns: world.towns, happenings };
}

const detailsWords = english ? enDetails : zhDetails;

/** "Claude · laptop" for a resident the World answered with; its id
 *  where it did not. */
function someoneAt(address: Address): string {
  const known = residentsOf(store.world.peek()).find((r) => r.about.id === address.resident && r.about.machine === address.machine);
  return known ? shellWords.someone(agentName(known.about.host.agent), known.about.machine) : `${address.resident} · ${address.machine}`;
}

/** The card about the agent in a conversation, and how to fill it. */
function detailsOf(address: Address, look: () => Promise<ResidentDetail | null>) {
  return {
    card: new Details({
      words: detailsWords,
      someone: someoneAt,
      link: talkLink,
      span: shellWords.span,
      stop: () => (live ? stop(address) : Promise.resolve(false)),
    }),
    label: detailsWords.open,
    look,
    colleagues: () =>
      residentsOf(store.world.peek()).filter(
        (r) =>
          r.about.origin.kind === "colleague" &&
          r.about.origin.asked_by.resident === address.resident &&
          r.about.origin.asked_by.machine === address.machine,
      ),
  };
}

function home(): Page {
  const page = new HomePage({
    words: homeWords,
    ended: words.ended,
    lastVisit: live ? seen.read() : specimen.lastVisit,
    link: talkLink,
    head: head(""),
    newTalk: () => newTalk.open(),
  });
  const stop = effect(() => {
    const world = store.world.get();
    const trouble = store.trouble.get();
    const now = store.clock.get();
    if (world) page.update(stateOf(world, live ? null : specimen.happenings), now);
    if (trouble) page.unreachable(trouble, now);
    if (live && document.visibilityState === "visible" && world) seen.now();
  });
  return {
    element: page.element,
    title: homeWords.world,
    leave() {
      stop();
      if (live) seen.now();
    },
  };
}

function specimenTalk(address: Address): Page {
  const name = address.resident;
  const sample = (specimens[name.startsWith("codex") ? "codex" : "claude"] ?? specimens.claude!)();
  const conversation = new Conversation();
  const known = residentsOf(specimen.world).find((r) => r.about.id === address.resident && r.about.machine === address.machine);
  const page = new ConversationPage({
    words,
    here: sample.here,
    someone: {
      agent: sample.agent === "claude" ? "Claude" : "Codex",
      machine: sample.machine,
      model: sample.model,
      town: sample.town,
    },
    head,
    link: talkLink,
    conversation,
    details: detailsOf(address, async () =>
      known
        ? {
            resident: known,
            records: `/home/lenovo/.fastctx/world/towns/${known.town ?? "outside"}/residents/${known.about.id}`,
            actions: known.last_action ? [known.last_action] : [],
            actions_total: known.last_action ? 41 : 0,
            asked: known.about.origin.kind === "colleague" ? known.about.origin.label : null,
            answers: [],
            jobs: [],
          }
        : null,
    ),
    send(text) {
      conversation.sending(text, sample.here);
      page.update();
    },
    stop() {},
  });
  let left = false;
  if (params.has("live")) {
    let index = 0;
    const next = () => {
      const event = sample.events[index++];
      if (!event || left) return;
      conversation.event(event);
      page.update();
      setTimeout(next, event.event === "heard" ? 900 : 420);
    };
    next();
  } else {
    conversation.history(sample.events);
    page.update();
  }
  return {
    element: page.element,
    title: sample.title,
    leave() {
      left = true;
      page.leave();
    },
  };
}

function liveTalk(address: Address): Page {
  const world = store.world.peek();
  const known = residentsOf(world).find((r) => r.about.id === address.resident && r.about.machine === address.machine);
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
    head,
    link: talkLink,
    conversation,
    details: detailsOf(address, async () => {
      const [found] = await look([address.resident]);
      const there = found?.found.find(
        (looked) => looked.kind === "resident" && looked.resident.about.machine === address.machine,
      );
      return there && there.kind === "resident" ? there : null;
    }),
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
  // What it started that still runs, as the World last answered.
  const running = effect(() => {
    const world = store.world.get();
    const now = store.clock.get();
    const jobs = jobsOf(world).filter((job) => job.owner.resident === address.resident && job.owner.machine === address.machine);
    page.running(jobs, now);
  });
  // Events can come before the history does; they wait for it.
  let ready = false;
  const early: Piece[] = [];
  const take = (piece: Piece) => {
    if (piece.kind === "event") conversation.event(piece.event);
    else page.lost(piece.error);
  };
  watch(address, (piece) => {
    if (left) return;
    if (!ready) early.push(piece);
    else {
      take(piece);
      page.update();
    }
  })
    .then((started) => {
      watchId = started.id;
      if (left) {
        void unwatch(started.id);
        return;
      }
      conversation.history(started.history);
      for (const piece of early) take(piece);
      ready = true;
      page.update();
    })
    .catch((error: unknown) => {
      page.lost(String(error));
      page.update();
    });
  return {
    element: page.element,
    title: `${someone.agent} · ${someone.machine}`,
    leave() {
      left = true;
      running();
      page.leave();
      if (watchId !== null) void unwatch(watchId);
    },
  };
}

function projectPage(town: string): Page {
  const page = new ProjectPage({
    words: projectWords,
    home: homeWords,
    town,
    head,
    newTalk: shellWords.newTalk,
    edit: shellWords.edit,
    begin: (town) => newTalk.open(town),
    revise: (town, record) => projectForm.open({ town, record }),
    startJournal: (town) => journalForm.open(town, null),
    link: talkLink,
    journalLink: (town, journal) => link({ kind: "journal", town, journal }),
  });
  if (!live) {
    const map = specimenMap();
    page.update(map, { machine: specimen.world.here, scope: { kind: "town", town, record: null }, machines: specimen.world.who.machines.map((machine) => machine.kind === "answered" ? { ...machine, reply: { ...machine.reply, residents: machine.reply.residents.filter((r) => r.town === town) } } : machine) }, specimen.now);
    return { element: page.element, title: map.record.name };
  }
  let timer = 0;
  let left = false;
  // Asked again every two seconds while in view, as the World is.
  const refresh = async () => {
    if (left) return;
    if (document.visibilityState === "visible") {
      try {
        const { map, who } = await readProject(town);
        if (!left) {
          shell.retitle(map.record.name);
          page.update(map, who, new Date());
        }
      } catch (error) {
        if (!left) page.unreachable(String(error));
      }
    }
    if (!left) timer = window.setTimeout(refresh, 2000);
  };
  void refresh();
  return {
    element: page.element,
    title: store.world.peek()?.towns.find((t) => t.town === town)?.record.name ?? town,
    leave() {
      left = true;
      clearTimeout(timer);
    },
  };
}

function journalPage(town: string, name: string): Page {
  const project = store.world.peek()?.towns.find((t) => t.town === town)?.record.name ?? town;
  const page = new JournalPage({
    words: projectWords,
    project: { href: link({ kind: "project", town }), name: project },
    journal: name,
    head,
    write: () => journalForm.open(town, name),
    earlier: (before) => (live ? readJournal(town, name, before) : Promise.reject(new Error("specimen"))),
  });
  if (!live) page.show(specimenJournal());
  else
    readJournal(town, name, null)
      .then((view) => page.show(view))
      .catch((error: unknown) => page.unreachable(String(error)));
  return { element: page.element, title: name };
}

function when(at: Date, now: Date): string {
  const sameDay = at.toDateString() === now.toDateString();
  return sameDay ? homeWords.clock(at) : `${homeWords.day(at)} ${homeWords.clock(at)}`;
}

function show(next: Place): void {
  place = next;
  let page: Page;
  switch (next.kind) {
    case "home":
      page = home();
      break;
    case "talk":
      page = live ? liveTalk(next.address) : specimenTalk(next.address);
      break;
    case "project":
      page = projectPage(next.town);
      break;
    case "journal":
      page = journalPage(next.town, next.journal);
      break;
    case "machines":
    case "machine":
      page = new MachinesPage({
        store,
        words: english ? enMachines : zhMachines,
        devices,
        deviceWords,
        head,
        focus: next.kind === "machine" ? next.name : null,
        when,
      });
      break;
    case "welcome":
      page = new WelcomePage({
        store,
        words: english ? enWelcome : zhWelcome,
        head,
        setup: () => (live ? setupState() : Promise.reject(new Error("specimen"))),
        place: () => (live ? placeFastctx() : notLive()),
        join: (code, name) => (live ? joinWorld(code, name) : notLive()),
        hubHere: (addresses) => (live ? hubHere(addresses) : notLive()),
        applyHosts: (codex, claude) => (live ? applyHosts(codex, claude, false, true) : notLive()),
        enter() {
          welcomed.now();
          location.hash = link({ kind: "home" });
        },
      });
      break;
    case "settings":
      page = new SettingsPage({
        store,
        words: english ? enSettings : zhSettings,
        head,
        language,
        chooseLanguage(chosen) {
          try {
            localStorage.setItem("lang", chosen);
          } catch {
            // Not kept: the language holds for this window only.
          }
          const url = new URL(location.href);
          url.searchParams.delete("lang");
          if (chosen === "en") url.searchParams.set("lang", "en");
          location.replace(url);
        },
        ago: (at, now) => homeWords.ago(now.getTime() - at.getTime()),
      });
      break;
  }
  shell.show(next, page);
}

/** In a plain browser, `?s=` names what to look at when the address does
 *  not. */
function specimenPlace(): Place | null {
  const asked = params.get("s");
  if (live || !asked || location.hash) return null;
  switch (asked) {
    case "home":
    case "new":
    case "new-project":
      return { kind: "home" };
    case "project":
      return { kind: "project", town: "mark50" };
    case "journal":
      return { kind: "journal", town: "mark50", journal: "q16-fixed-point" };
    case "machines":
      return { kind: "machine", name: "laptop" };
    case "invite":
      return { kind: "machines" };
    case "welcome":
      return { kind: "welcome" };
    case "settings":
      return { kind: "settings" };
    case "codex":
      return { kind: "talk", address: { resident: "codex-7f3a", machine: "laptop" } };
    default:
      return { kind: "talk", address: { resident: "claude-2b1c", machine: "laptop" } };
  }
}

const paletteWords = english ? enPalette : zhPalette;
const themeNames = (english ? enSettings : zhSettings).themes;
new Palette({
  words: paletteWords,
  items() {
    const world = store.world.peek();
    const go = (to: Place) => () => {
      location.hash = link(to);
    };
    const items: PaletteItem[] = [
      { group: paletteWords.actions, label: shellWords.newTalk, note: "Ctrl N", run: () => newTalk.open(place.kind === "project" ? place.town : null) },
      { group: paletteWords.actions, label: shellWords.newProject, run: () => projectForm.open() },
      { group: paletteWords.actions, label: deviceWords.invite, run: () => devices.invite() },
      { group: paletteWords.actions, label: paletteWords.fold, note: "Ctrl B", run: () => shell.toggleSidebar() },
      ...themes.map((theme) => ({ group: paletteWords.actions, label: paletteWords.theme(themeNames[theme]), run: () => chooseTheme(theme) })),
      { group: paletteWords.places, label: shellWords.world, run: go({ kind: "home" }) },
      ...(world?.towns ?? []).map((town) => ({ group: paletteWords.places, label: town.record.name, note: town.town, run: go({ kind: "project", town: town.town }) })),
      { group: paletteWords.places, label: shellWords.settings, run: go({ kind: "settings" }) },
      ...residentsOf(world)
        .filter((r) => r.here || r.now?.waiting_for_user)
        .map((r) => ({
          group: paletteWords.agents,
          label: r.about.origin.kind === "colleague" ? r.about.origin.label : shellWords.someone(agentName(r.about.host.agent), r.about.machine),
          note: [r.about.origin.kind === "colleague" ? shellWords.someone(agentName(r.about.host.agent), r.about.machine) : null, r.town].filter(Boolean).join(" · "),
          run: go({ kind: "talk", address: { resident: r.about.id, machine: r.about.machine } }),
        })),
      ...(world?.nodes.machines ?? [])
        .filter((node) => !node.revoked)
        .map((node) => ({ group: paletteWords.machines, label: node.name, note: node.online ? "" : shellWords.offline, run: go({ kind: "machine", name: node.name }) })),
    ];
    return items;
  },
});

/** Whether this device has been shown the way into a World: a computer
 *  on its own is shown it once, by itself, and after that from the
 *  sidebar. */
const welcomed = {
  read(): boolean {
    try {
      return localStorage.getItem("welcomed") === "1";
    } catch {
      return false;
    }
  },
  now(): void {
    try {
      localStorage.setItem("welcomed", "1");
    } catch {
      // Not kept: it is shown again next time.
    }
  },
};

if (live) {
  // Decided once, on the World's first answer.
  let decided = false;
  effect(() => {
    const world = store.world.get();
    if (!world || decided) return;
    decided = true;
    if (!world.nodes.world && !welcomed.read() && place.kind === "home") location.hash = link({ kind: "welcome" });
  });
}

installPens();
installRules();
addEventListener("hashchange", () => show(placeOf(location.hash)));
applyTheme(() => {
  void redrawDiagrams(shell.element);
});
show(specimenPlace() ?? placeOf(location.hash));
if (!live && params.get("s") === "new") newTalk.open();
if (!live && params.get("s") === "new-project") projectForm.open();
if (!live && params.get("s") === "invite") devices.invite();
