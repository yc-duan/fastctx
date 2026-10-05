// The interface's entry: the frame with its sidebar, and the place the
// address names in it (`app/places.ts`). Inside the shell it reads the
// World on this machine; in a plain browser a specimen stands in, and
// `?s=home|claude|codex|project|journal|machines|settings|new`,
// `?theme=…`, `?lang=en` and `?live` pick what to look at.

import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { Attachment, UploadTarget } from "./world/attachments";
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
import "./pages/pages.css";

import { Devices } from "./app/devices";
import { MachinesPage } from "./app/machines";
import { NewTalk } from "./app/new-talk";
import { Palette, type PaletteItem } from "./app/palette";
import { ProjectForm } from "./app/project-form";
import { link, placeOf, talkLink, type Place } from "./app/places";
import { SettingsPage, legacySaid } from "./app/settings";
import { ReaderPage } from "./app/reader";
import { WelcomePage } from "./app/welcome";
import { Shell, pageHead, type Page } from "./app/shell";
import { effect } from "./app/signals";
import { createStore, jobsOf, residentsOf } from "./app/store";
import {
  enComposer,
  enPresence,
  enDetails,
  enDevices,
  enMachines,
  enNewTalk,
  enPalette,
  enProjectForm,
  enSettings,
  enShell,
  enWelcome,
  zhComposer,
  zhPresence,
  zhDetails,
  zhDevices,
  zhMachines,
  zhNewTalk,
  zhPalette,
  zhProjectForm,
  zhSettings,
  zhShell,
  zhWelcome, readerWords } from "./app/words";
import { Details, type Manage } from "./conversation/details";
import { Conversation } from "./conversation/model";
import { ConversationPage, type Someone } from "./conversation/page";
import { redrawDiagrams } from "./conversation/diagrams";
import { HomePage } from "./home/page";
import type { Away, Happening, WorldState } from "./home/model";
import { happeningsOf } from "./home/since";
import { enHome, zhHome } from "./home/words";
import { installPens } from "./pen/filters";
import { installRules } from "./pen/rules";
import { JournalPage } from "./project/journal";
import { JournalForm } from "./project/journal-form";
import { ProjectPage } from "./project/page";
import { enProject, zhProject } from "./project/words";
import { specimenJournal, specimenMap } from "./specimen/project";
import { markdownOf } from "./conversation/export";
import { specimens } from "./specimen/sessions";
import { specimenAgents, specimenNow } from "./specimen/world";
import { specimenComputer, specimenFiles, specimenOffers, specimenRead, specimenUpload } from "./specimen/seams";
import { applyTheme, chooseTheme, currentTheme, themes } from "./theme";
import { Confirm } from "./app/confirm";
import { PageScreen } from "./pages/view";
import { manage as manageResident, type Management } from "./world/management";
import { toolsFor } from "./pages/tools";
import { enPages, zhPages } from "./pages/words";
import { act as pageAct, consent as pageConsent, pageHistory, readPage, restore as restorePage, withdraw as withdrawPage } from "./world/pages";
import { specimenPageTools, specimenPageWorld } from "./specimen/pages";
import type { Address } from "./world/events";
import { ownerResident, type ResidentDetail } from "./world/world";
import {
  applyHosts,
  control,
  hubHere,
  offers,
  invite,
  joinWorld,
  placeFastctx,
  setupState,
  onComputer,
  connectionState,
  joinDevice,
  canScan,
  scanInvitation,
  readText,
  uploadBytes,
  uploadPath,
  readBytes,
  keepHere,
  releaseHere,
  journal as readJournal,
  journalAdd,
  leave as leaveWorld,
  look,
  projectEdit,
  projectNew,
  agents,
  attachHere,
  open as openAgent,
  openOutside,
  since,
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
import { desktopAvailable } from "./world/desktop";
import { startPresence } from "./app/presence";
import { installAccess } from "./app/access";
import { installFrame } from "./app/frame";
import { nameCodeCopy } from "./conversation/prose";

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
const pageWords = english ? enPages : zhPages;
const shellWords = english ? enShell : zhShell;
document.documentElement.lang = english ? "en" : "zh-CN";

const specimen = specimenNow();
const store = createStore(() => ({ world: specimen.world, now: specimen.now }));
const live = store.live;
/** Whether this interface reaches the World as a device (a phone, or a
 *  computer started with --device): it joins as one and sets nothing up.
 *  A specimen shows one when asked. The shell's own answer follows. */
let asDevice = live ? !onComputer() : params.has("phone");

const head = (title: string, actions?: string) => pageHead(shellWords, title, actions);

let place: Place = { kind: "home" };

const newTalk = new NewTalk({
  store,
  words: english ? enNewTalk : zhNewTalk,
  agents: (machine) => (live ? agents(machine) : Promise.resolve(specimenAgents(machine))),
  async begin({ agent, machine, town, text, model, effort, attachments }) {
    if (!live) throw new Error(english ? "this is a specimen" : "这是样例，没有连着世界");
    const { to } = await openAgent(agent, machine, town, text, model, effort, attachments);
    void store.refresh();
    return to;
  },
  opened(address) {
    location.hash = talkLink(address);
  },
  attach: {
    words: (english ? enComposer : zhComposer).attach,
    async upload(machine, target, source, progress) {
      if (!live) return specimenUpload(machine, target, source, progress);
      if (source.kind === "path") return uploadPath(machine, target, source.path, progress);
      const done = await uploadBytes(machine, target, source.file);
      progress(source.file.size);
      return done;
    },
    ...(live ? { drops: windowDrops } : {}),
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
  invite: (name, hours, device) =>
    live ? invite(name, hours, device) : Promise.resolve(`fxw3.${"specimen-not-a-real-invitation-".repeat(20)}${name ?? ""}${hours}`),
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

function stateOf(world: WorldNow, away: Away | null): WorldState {
  return { here: world.here, machines: world.who.machines, nodes: world.nodes.machines, towns: world.towns, away };
}

/** The parts of the World's present whose change leaves something behind
 *  to tell: a turn ending, a job going, a machine coming or going, a
 *  journal growing, someone arriving. */
function shapeOf(world: WorldNow): string {
  return JSON.stringify([
    residentsOf(world).map((r) => [r.about.id, r.about.machine, r.now?.activity ?? null, r.now?.turn ?? null, r.town]),
    jobsOf(world).map((job) => job.job),
    world.nodes.machines.map((node) => [node.name, node.online]),
    world.towns.map((town) => [town.town, town.journals]),
  ]);
}

/** The first line of each journal entry in the chronicle, read from the
 *  journal on this machine; where it cannot be read, the line goes
 *  without it. */
async function withFirstLines(happenings: Happening[]): Promise<Happening[]> {
  const journals = new Map<string, Promise<Map<string, string>>>();
  const firsts = (town: string, name: string) => {
    const key = `${town}/${name}`;
    let read = journals.get(key);
    if (!read) {
      read = readJournal(town, name, null).then(
        (view) => new Map(view.entries.map((entry) => [entry.stamp.id, entry.text.trim().split("\n")[0] ?? ""])),
        () => new Map<string, string>(),
      );
      journals.set(key, read);
    }
    return read;
  };
  return Promise.all(
    happenings.map(async (happening) =>
      happening.kind === "journal" ? { ...happening, first: (await firsts(happening.town, happening.journal)).get(happening.entry) ?? null } : happening,
    ),
  );
}

const detailsWords = english ? enDetails : zhDetails;
const composerWords = english ? enComposer : zhComposer;

/** Files dropped onto the window from the system, by path, as the shell
 *  hands them over; returns how to stop listening. */
function windowDrops(on: { over(over: boolean): void; dropped(paths: string[]): void }): () => void {
  let stop: (() => void) | null = null;
  let left = false;
  void getCurrentWebview()
    .onDragDropEvent(({ payload }) => {
      if (payload.type === "enter" || payload.type === "over") on.over(true);
      else if (payload.type === "leave") on.over(false);
      else {
        on.over(false);
        on.dropped(payload.paths);
      }
    })
    .then((unlisten) => {
      if (left) unlisten();
      else stop = unlisten;
    });
  return () => {
    left = true;
    stop?.();
  };
}

/** A conversation as Markdown, named as its page names it. */
function exported(conversation: Conversation, someone: Someone): () => string {
  return () =>
    markdownOf(conversation, words, {
      title: conversation.title ?? `${someone.agent} · ${someone.machine}`,
      agent: someone.agent,
      you: homeWords.you,
      plan: words.plan,
      waiting: words.waitingForYou,
    });
}

/** "Claude · laptop" for a resident the World answered with; its id
 *  where it did not. */
function someoneAt(address: Address): string {
  const known = residentsOf(store.world.peek()).find((r) => r.about.id === address.resident && r.about.machine === address.machine);
  return known ? shellWords.someone(agentName(known.about.host.agent), known.about.machine) : `${address.resident} · ${address.machine}`;
}

/** The card about the agent in a conversation, and how to fill it. */
function detailsOf(address: Address, look: () => Promise<ResidentDetail | null>, markdown?: () => string) {
  return {
    card: new Details({
      words: detailsWords,
      someone: someoneAt,
      link: talkLink,
      span: shellWords.span,
      stop: () => (live ? stop(address) : Promise.resolve(false)),
      here: () => store.world.peek()?.here ?? (live ? null : specimen.world.here),
      // A specimen opens nothing; its button only shows where it would be.
      attach: desktopAvailable()
        ? () => attachHere(address.resident)
        : live
          ? undefined
          : () => new Promise<void>((resolve) => setTimeout(resolve, 2400)),
      markdown,
      manage: live ? liveManage(address) : specimenManage(),
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

/** The projects an agent on `machine` can be in: those spanning it. */
function townsOn(machine: string): { town: string; name: string }[] {
  return (store.world.peek()?.towns ?? [])
    .filter((town) => town.record.machines.includes(machine))
    .map((town) => ({ town: town.town, name: town.record.name }));
}

/** Managing an agent for the user: each act is the World's, and whatever
 *  stage of it was not done is said in the card. */
function liveManage(address: Address): Manage {
  const act = async (action: Management) => {
    const managed = await manageResident(address, action);
    void store.refresh();
    return managed.steps
      .filter((step): step is typeof step & { outcome: "pending" | "unsupported" | "failed" } => step.outcome !== "done")
      .map((step) => detailsWords.unfinished(step.stage, step.outcome, step.detail));
  };
  return {
    sleep: () => act({ do: "sleep" }),
    wake: () => act({ do: "wake" }),
    move: (town) => act({ do: "move", town }),
    remove: () => act({ do: "remove" }),
    towns: townsOn,
    removed: () => {
      location.hash = link({ kind: "home" });
    },
  };
}

/** A specimen's agent can be managed only to look at: each act says it is
 *  a specimen, after a moment. */
function specimenManage(): Manage {
  const refuse = () =>
    new Promise<string[]>((_, reject) =>
      setTimeout(() => reject(new Error(english ? "this is a specimen" : "这是样例，没有连着世界")), 700),
    );
  return {
    sleep: refuse,
    wake: refuse,
    move: refuse,
    remove: refuse,
    towns: townsOn,
    removed: () => {
      location.hash = link({ kind: "home" });
    },
  };
}

function home(): Page {
  const lastVisit = live ? seen.read() : specimen.lastVisit;
  const page = new HomePage({
    words: homeWords,
    ended: words.ended,
    lastVisit,
    link: talkLink,
    head: head(""),
    newTalk: () => newTalk.open(),
  });
  let away: Away | null = live ? null : specimen.away;
  // What happened while the reader was away is asked of every machine when
  // the page opens, and again whenever the present changes in a way that
  // leaves something to tell; one question at a time.
  let shape = "";
  let asking = false;
  let again = false;
  const ask = async (): Promise<void> => {
    if (!lastVisit) return;
    if (asking) {
      again = true;
      return;
    }
    asking = true;
    try {
      const read = happeningsOf(await since(lastVisit, null));
      away = { happenings: await withFirstLines(read.happenings), unasked: read.unasked, trouble: null };
    } catch (error) {
      away = { happenings: away?.happenings ?? [], unasked: [], trouble: String(error) };
    }
    asking = false;
    const world = store.world.peek();
    if (world) page.update(stateOf(world, away), store.clock.peek());
    if (again) {
      again = false;
      void ask();
    }
  };
  const stop = effect(() => {
    const world = store.world.get();
    const trouble = store.trouble.get();
    const now = store.clock.get();
    if (world) page.update(stateOf(world, away), now);
    if (live && world) {
      const next = shapeOf(world);
      if (next !== shape) {
        shape = next;
        void ask();
      }
    }
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
  const sample = (specimens[name === "codex-a51c" ? "asking" : name.startsWith("codex") ? "codex" : "claude"] ?? specimens.claude!)();
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
    controls: {
      words: composerWords,
      offers: async () => specimenOffers,
      control: async () => ({ kind: "done", said: english ? "this is a specimen" : "这是样例，没有连着世界" }),
      forked: () => {},
      open: openOutside,
    },
    // A specimen carries nothing anywhere: an attachment arrives after a
    // moment, so the tray can be looked at.
    attach: {
      words: composerWords.attach,
      seam: {
        upload: (source, name, progress) =>
          new Promise((resolve) => {
            const size = source.kind === "bytes" ? source.file.size : 0;
            let sent = 0;
            const step = () => {
              sent = Math.min(size, sent + Math.max(1, size / 8));
              progress(sent);
              if (sent >= size) resolve({ specimen: name });
              else setTimeout(step, 120);
            };
            step();
          }),
      },
    },
    draftKey: `draft:${address.resident}@${address.machine}`,
    files: { words: composerWords.files, find: specimenFiles },
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
      exported(conversation, { agent: sample.agent === "claude" ? "Claude" : "Codex", machine: sample.machine, model: sample.model, town: sample.town }),
    ),
    send(text, attached) {
      conversation.sending(
        text,
        sample.here,
        attached.shown.map(({ name, size, image, preview }) => ({ name, size, image, preview })),
      );
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
    controls: {
      words: composerWords,
      offers: () => offers(address),
      control: (what) => control(address, what),
      forked: (resident) => {
        void store.refresh();
        location.hash = talkLink(resident);
      },
      open: openOutside,
    },
    draftKey: `draft:${address.resident}@${address.machine}`,
    attach: {
      words: composerWords.attach,
      seam: {
        upload: async (source, _name, progress) => {
          const target: UploadTarget = { kind: "resident", resident: address.resident };
          if (source.kind === "path") return uploadPath(address.machine, target, source.path, progress);
          const done = await uploadBytes(address.machine, target, source.file);
          progress(source.file.size);
          return done;
        },
      },
      drops: windowDrops,
    },
    details: detailsOf(address, async () => {
      const [found] = await look([address.resident]);
      const there = found?.found.find(
        (looked) => looked.kind === "resident" && looked.resident.about.machine === address.machine,
      );
      return there && there.kind === "resident" ? there : null;
    }, exported(conversation, someone)),
    send(text, attached) {
      const key = conversation.sending(
        text,
        here,
        attached.shown.map(({ name, size, image, preview }) => ({ name, size, image, preview })),
      );
      page.update();
      return talk(address, text, attached.refs as Attachment[])
        .then(({ to, landed }) => {
          // Words in a turn take their place when the session hears them;
          // words in no turn yet say where they are.
          if (landed.kind === "waiting" || landed.kind === "sent" || landed.kind === "uncertain") {
            const why = "why" in landed ? landed.why : null;
            conversation.sent(key, { state: "landed", how: landed.kind, machine: to.machine, why });
            page.update();
          }
          // A turn whose end this conversation will not see leaves nothing
          // for words queued after these to wait on.
          return landed.kind === "uncertain" || (landed.kind === "waiting" && !landed.answered) ? ("unseen" as const) : ("turn" as const);
        })
        .catch((error: unknown) => {
          conversation.sent(key, { state: "failed", why: String(error) });
          page.update();
          return "failed" as const;
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
    const jobs = jobsOf(world).filter((job) => {
      const owner = ownerResident(job.owner);
      return owner?.resident === address.resident && owner.machine === address.machine;
    });
    page.running(jobs, now);
  });
  const take = (piece: Piece) => {
    if (piece.kind === "event") conversation.event(piece.event);
    else page.lost(piece.error);
  };
  // Each watch is one reading of the session; when the host replaces or
  // removes turns, what is shown is read again from a fresh one.
  let reading = 0;
  const read = () => {
    const mine = ++reading;
    // Events can come before the history does; they wait for it.
    let ready = false;
    const early: Piece[] = [];
    const settle = () => {
      if (conversation.stale) {
        if (watchId !== null) void unwatch(watchId);
        watchId = null;
        read();
      }
      page.update();
    };
    watch(address, (piece) => {
      if (left || mine !== reading) return;
      if (!ready) early.push(piece);
      else {
        take(piece);
        settle();
      }
    })
      .then((started) => {
        if (left || mine !== reading) {
          void unwatch(started.id);
          return;
        }
        watchId = started.id;
        conversation.history(started.history);
        for (const piece of early) take(piece);
        ready = true;
        settle();
      })
      .catch((error: unknown) => {
        if (mine !== reading) return;
        page.lost(String(error));
        page.update();
      });
  };
  read();
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
    pageWords,
    pageLink: (town, page) => link({ kind: "page", town, page, version: null }),
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

/** The page open now, to hear the theme change. */
let openPage: PageScreen | null = null;
const pageConfirm = new Confirm({ close: pageWords.close, working: pageWords.working, failed: pageWords.failed });

function pageScreen(town: string, name: string, version: number | null): Page {
  const project = store.world.peek()?.towns.find((t) => t.town === town)?.record.name ?? town;
  const screen = new PageScreen({
    words: pageWords,
    page: name,
    version,
    project: { href: link({ kind: "project", town }), name: project },
    head,
    link: (version) => link({ kind: "page", town, page: name, version }),
    ago: (at) => homeWords.ago((live ? Date.now() : specimen.now.getTime()) - at.getTime()),
    someone: someoneAt,
    world: live
      ? {
          read: (version) => readPage(town, name, version),
          history: () => pageHistory(town, name),
          consent: (version, base, acts) => pageConsent(town, name, version, base, acts),
          withdraw: (base) => withdrawPage(town, name, base),
          restore: (base, version) => restorePage(town, name, base, version),
        }
      : specimenPageWorld(name),
    tools: (view) => {
      const tools = toolsFor({
        town,
        store,
        open: (address) => (location.hash = talkLink(address)),
        act: (action) => pageAct(town, name, view.publication.version, action),
      });
      return live ? tools : specimenPageTools(view, tools);
    },
    context: () => ({ theme: currentTheme(), locale: english ? "en" : "zh-CN", platform: asDevice ? "mobile" : "desktop" }),
    version_: "1",
    confirm: pageConfirm,
    retitle: (title) => shell.retitle(title),
    changed: () => {},
  });
  openPage = screen;
  return {
    element: screen.element,
    title: name,
    leave() {
      screen.leave();
      if (openPage === screen) openPage = null;
    },
  };
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
    case "page":
      page = pageScreen(next.town, next.page, next.version);
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
        agents: () =>
          live
            ? agents(null)
            : Promise.resolve({
                machine: specimen.world.here,
                machines: specimen.world.nodes.machines
                  .filter((node) => node.online)
                  .flatMap((node) => specimenAgents(node.name).machines),
              }),
        applyHere: (agent, into) =>
          live ? applyHosts(agent === "codex", agent === "claude", false, into) : Promise.reject(new Error(english ? "this is a specimen" : "这是样例，没有连着世界")),
      });
      break;
    case "welcome":
      page = new WelcomePage({
        store,
        words: english ? enWelcome : zhWelcome,
        head,
        device: asDevice,
        setup: () => (live ? setupState() : Promise.reject(new Error("specimen"))),
        legacy: (legacy) => legacySaid(legacy, english ? enSettings : zhSettings),
        place: () => (live ? placeFastctx() : notLive()),
        join: (code, name) =>
          live && asDevice
            ? joinDevice(code, name).then((joined) => ({ ...joined, kept: null }))
            : (live ? joinWorld(code, name) : notLive()),
        // A phone reads the invitation off the computer's screen; a
        // specimen shows the button when asked as a phone.
        scan: canScan() ? scanInvitation : !live && params.has("phone") ? async () => "fxw3.specimen" : undefined,
        hubHere: (addresses) => (live ? hubHere(addresses) : notLive()),
        applyHosts: (codex, claude) => (live ? applyHosts(codex, claude, false, true) : notLive()),
        enter() {
          welcomed.now();
          location.hash = link({ kind: "home" });
        },
      });
      break;
    case "file":
      page = new ReaderPage({
        words: readerWords[english ? "en" : "zh"],
        machine: next.machine,
        path: next.path,
        tail: next.tail,
        head,
        read: (machine, path, range) => (live ? readText(machine, path, range) : specimenRead(machine, path, range)),
        readBytes: (machine, path) => (live ? readBytes(machine, path) : Promise.reject(new Error(deviceWords.specimen))),
        ago: (at) => homeWords.ago(Date.now() - at.getTime()),
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
        computer: live ? (onComputer() ? { setup: setupState, keep: keepHere, release: releaseHere } : undefined) : specimenComputer(),
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
    case "page":
      return { kind: "page", town: "mark50", page: "tuning", version: null };
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
    case "asks":
      return { kind: "talk", address: { resident: "codex-a51c", machine: "laptop" } };
    case "decision":
      return { kind: "file", machine: "desktop", path: "D:/work/mark50/specs/控制环.md", tail: false };
    case "log":
      return { kind: "file", machine: "lab", path: "/home/lenovo/.fastctx/world/towns/mark50/residents/codex-3c77/out/j-p0q1r2.log", tail: true };
    case "about":
      // On this machine, so the card offers its own interface too.
      return { kind: "talk", address: { resident: "codex-5e2a", machine: "desktop" } };
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
      ...(place.kind === "talk"
        ? [
            {
              group: paletteWords.actions,
              label: words.find.find,
              note: "Ctrl F",
              run: () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "f", ctrlKey: true })),
            },
          ]
        : []),
      ...themes.map((theme) => ({ group: paletteWords.actions, label: paletteWords.theme(themeNames[theme]), run: () => chooseTheme(theme) })),
      { group: paletteWords.places, label: shellWords.world, run: go({ kind: "home" }) },
      ...(world?.towns ?? []).map((town) => ({ group: paletteWords.places, label: town.record.name, note: town.town, run: go({ kind: "project", town: town.town }) })),
      { group: paletteWords.places, label: shellWords.machines, run: go({ kind: "machines" }) },
      { group: paletteWords.places, label: shellWords.settings, run: go({ kind: "settings" }) },
      { group: paletteWords.places, label: homeWords.setUp, run: go({ kind: "welcome" }) },
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
  // A computer the World cannot be asked on at all, before any answer: when
  // fastctx is not on it yet, the way on is to put it there.
  effect(() => {
    const trouble = store.trouble.get();
    if (!trouble || decided || store.world.peek()) return;
    decided = true;
    void setupState()
      .then((setup) => {
        if (!setup.installed && place.kind === "home") location.hash = link({ kind: "welcome" });
      })
      .catch(() => {});
  });
}

installPens();
installRules();
installAccess();
// A specimen shows the window's frame when asked, to be looked at.
installFrame(shellWords.frame, !live && params.has("frame"));
nameCodeCopy(words.copyCode);
addEventListener("hashchange", () => show(placeOf(location.hash)));
applyTheme(() => {
  void redrawDiagrams(shell.element);
  openPage?.contextChanged();
});
show(specimenPlace() ?? placeOf(location.hash));
// A device that has not joined has nothing to show but the way in.
if (live) {
  void connectionState().then((state) => {
    asDevice = state.mode === "device";
    if (asDevice && !state.device?.world && place.kind === "home") location.hash = link({ kind: "welcome" });
    else if (place.kind === "welcome") show(place);
  });
}
if (!live && params.get("s") === "new") newTalk.open();
if (!live && params.get("s") === "new-project") projectForm.open();
if (!live && params.get("s") === "invite") devices.invite();

startPresence({
  store,
  words: english ? enPresence : zhPresence,
  someone: (r) => shellWords.someone(agentName(r.about.host.agent), r.about.machine),
  stop: (address) => stop(address),
});

