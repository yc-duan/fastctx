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
import "./conversation/turns.css";
import "./conversation/model-panel.css";
import "./longview/longview.css";
import "./app/shell.css";
import "./app/sheet.css";
import "./app/pages.css";
import "./app/accounts.css";
import "./app/machines.css";
import "./app/agents.css";
import "./app/features.css";
import "./app/removal.css";
import "./app.css";
import "./home/home.css";
import "./project/project.css";
import "./pages/pages.css";

import { AccountPage } from "./app/account";
import { AdminPage } from "./app/admin";
import { LoginPage } from "./app/login";
import { MachinesPage } from "./app/machines";
import { AgentsPage } from "./app/agents";
import { enAgents, zhAgents } from "./app/agents-words";
import { TalksPage, enTalks, zhTalks } from "./app/talks";
import { FeaturesPage } from "./app/features";
import { enFeatures, zhFeatures } from "./app/features-words";
import { accountOf, enHomes, homeChoice, homeLabel, homesOf, installed, loggedOut, zhHomes } from "./app/agent-homes";
import { Tokens } from "./app/tokens";
import { NewTalk } from "./app/new-talk";
import { Palette, type PaletteItem } from "./app/palette";
import { ProjectForm } from "./app/project-form";
import { link, placeOf, talkLink, type Place } from "./app/places";
import { SettingsPage } from "./app/settings";
import { ReaderPage } from "./app/reader";
import { Shell, pageHead, type Page } from "./app/shell";
import { effect, signal } from "./app/signals";
import { addressOf, createStore, jobsOf, residentAt, residentsOf, sameAddress } from "./app/store";
import {
  enComposer,
  enConsent,
  enPresence,
  enDetails,
  enNewTalk,
  enPalette,
  enProjectForm,
  enSettings,
  enShell,
  zhComposer,
  zhConsent,
  zhPresence,
  zhDetails,
  zhNewTalk,
  zhPalette,
  zhProjectForm,
  zhSettings,
  zhShell,
  readerWords } from "./app/words";
import { enRemoval, zhRemoval } from "./app/removal-words";
import { enAccess, enAccount, enAdmin, enLogin, enMachines, enToken, zhAccess, zhAccount, zhAdmin, zhLogin, zhMachines, zhToken } from "./app/account-words";
import { loginsOf, type AccountChange, type Login } from "./world/accounts";
import { borrowedBy, borrowedOf, filePlaces, groupsOf, loginLabel, machinesOf, sameLogin, workPlaces, type Machine } from "./app/reach";
import { Details, type Manage } from "./conversation/details";
import { ComposerSession } from "./conversation/composer";
import { attachmentThumbnail, fetchFile, openLocal, reveal, thumbnail, thumbnailUrl } from "./world/files";
import { specimenComposer } from "./specimen/composer";
import { ClaudeConsent } from "./app/claude-consent";
import { Conversation } from "./conversation/model";
import { ConversationPage, type Someone } from "./conversation/page";
import { redrawDiagrams } from "./conversation/diagrams";
import { HomePage } from "./home/page";
import type { Away, Happening, WorldState } from "./home/model";
import { happeningsOf } from "./home/since";
import { enHome, zhHome } from "./home/words";
import { installHints } from "./app/hint";
import { installPens } from "./pen/filters";
import { installRules } from "./pen/rules";
import { JournalPage } from "./project/journal";
import { JournalForm } from "./project/journal-form";
import { ProjectPage } from "./project/page";
import { enProject, zhProject } from "./project/words";
import { specimenJournal, specimenMap } from "./specimen/project";
import { markdownOf } from "./conversation/export";
import { specimens } from "./specimen/sessions";
import { specimenAgents, specimenChange, specimenNow } from "./specimen/world";
import { specimenClaudeOffers, specimenComputer, specimenFiles, specimenOffers, specimenOutput, specimenPhone, specimenPicture, specimenRead, specimenUpload } from "./specimen/seams";
import { RemoteLines } from "./world/remote-lines";
import { applyTheme, chooseTheme, currentTheme, themes } from "./theme";
import { Confirm } from "./app/confirm";
import { showPathNotice } from "./app/path-notice";
import { PageScreen } from "./pages/view";
import { manage as manageResident, type Management } from "./world/management";
import { FileSearch } from "./world/file-search";
import { configurePhone, onPhone, phoneSeam, routeActivations } from "./app/phone";
import { renewalState, renewHere, renewGuardianHere, type VersionMismatch } from "./world/renewal";
import { toolsFor } from "./pages/tools";
import { enPages, zhPages } from "./pages/words";
import { act as pageAct, consent as pageConsent, pageHistory, readPage, restore as restorePage, withdraw as withdrawPage } from "./world/pages";
import { specimenPageTools, specimenPageWorld } from "./specimen/pages";
import type { Address } from "./world/events";
import { ownerResident, type ResidentDetail } from "./world/world";
import { agentName, titleOf } from "./world/names";
import { mapOf } from "./app/map-of";
import { NetworkMap } from "./app/network-map";
import {
  applyHosts,
  control,
  offers,
  placeFastctx,
  setupState,
  onComputer,
  connectionState,
  canScan,
  scanToken,
  readText,
  uploadBytes,
  uploadPath,
  readBytes,
  keepHere,
  removalPlan,
  removeHere,
  removeApp,
  retentionRead,
  retentionWrite,
  journal as readJournal,
  journalAdd,
  look,
  projectEdit,
  projectNew,
  agents,
  attachHere,
  claudeConsent,
  open as openAgent,
  openOutside,
  since,
  project as readProject,
  stop,
  stopJob,
  talk,
  unwatch,
  watch,
  type Piece,
  type WorldNow,
  login,
  loginToken,
  openAccount,
  createWorld,
  hereAddresses,
  accountChange,
  agentHomes,
  serverCommand,
  setWorksOn,
} from "./world/bridge";
import { en, zh } from "./words";
import { updating } from "./world/updater";
import { desktopAvailable } from "./world/desktop";
import { notifying, startPresence } from "./app/presence";
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
const agentHomeWords = english ? enHomes : zhHomes;
const projectWords = english ? enProject : zhProject;
const pageWords = english ? enPages : zhPages;
const presenceWords = english ? enPresence : zhPresence;
const shellWords = english ? enShell : zhShell;
document.documentElement.lang = english ? "en" : "zh-CN";

const specimen = specimenNow(params.get("accounts") === "2");
// `?accounts=0`: a computer in no World yet, as on its first open.
if (params.get("accounts") === "0") {
  specimen.world.accounts.hubs = [];
  specimen.world.nodes.hubs = [];
}
const store = createStore(() => ({ world: specimen.world, now: specimen.now }));
const live = store.live;
/** Whether this interface reaches the World as a device (a phone, or a
 *  computer started with --device): it joins as one and sets nothing up.
 *  A specimen shows one when asked. The shell's own answer follows. */
let asDevice = live ? !onComputer() : params.has("phone");

const head = (title: string, actions?: string) => pageHead(shellWords, title, actions);

let place: Place = { kind: "home" };

/** A local protocol mismatch reported independently by the carried CLI: the
 *  node refuses an interface of another protocol, and the reason is asked
 *  of the program that can still read it. */
const oldWorld = signal<VersionMismatch | null>(null);
if (live && onComputer()) {
  void renewalState().then(status => showPathNotice(status, english ? enSettings : zhSettings, pageWords.close));
  effect(() => {
    if (!store.trouble.get()) {
      oldWorld.set(null);
      return;
    }
    // Where it cannot be read either, the refusal itself is what is said.
    void renewalState().then((status) => oldWorld.set(status.mismatch), () => {});
  });
}

// Letting Claude work without asking on a machine: only the user's press,
// on one of the user's devices, writes it there.
const consent = new ClaudeConsent(english ? enConsent : zhConsent, (machine, accept, home) =>
  live ? claudeConsent(machine, accept, home) : Promise.reject(new Error(english ? "this is a specimen" : "这是样例，没有连着世界")),
);

/** What a machine's own programs say of its agents, or with null every
 *  machine's this computer reaches, as a live World or the specimen
 *  answers. */
function agentsOn(machine: string | null) {
  if (live) return agents(machine);
  const names = machine
    ? [machine]
    : [...new Set([...machinesOf(specimen.world).filter((one) => one.here || one.online).map((one) => one.name), ...borrowedOf(specimen.world).map((one) => one.machine)])];
  return Promise.resolve({ machine: specimen.world.here, machines: names.flatMap((name) => specimenAgents(name, specimen.now).machines) });
}

/** The logins of `agent` on a machine, named or as written, as choices:
 *  each home's directory, "" for the default one, with who is logged in. */
async function loginsOn(machine: string, agent: string) {
  const named = machinesOf(store.world.peek() ?? (live ? null : specimen.world)).find((m) => m.name === machine || m.written === machine)?.name ?? machine;
  const answer = (await agentsOn(named)).machines.find((m) => m.kind === "answered" && m.reply.machine === named);
  const facts = answer?.kind === "answered" ? answer.reply.agents : [];
  return homesOf(facts, agent)
    .filter(installed)
    .map((home) => ({
      value: homeChoice(home) ?? "",
      label: homeLabel(home, agentHomeWords),
      note: loggedOut(home) ? (english ? enNewTalk : zhNewTalk).loggedOut : (accountOf(home).plan ?? undefined),
      disabled: loggedOut(home),
    }));
}

const newTalk = new NewTalk({
  store,
  words: english ? enNewTalk : zhNewTalk,
  homeWords: agentHomeWords,
  agents: (machine) => agentsOn(machine),
  async begin({ agent, machine, town, text, model, effort, attachments, account, worksOn, home, filesOn }) {
    if (!live) throw new Error(specimenSaid);
    const { to } = await openAgent(agent, machine, town, text, model, effort, attachments, {
      ...(account ? { account } : {}),
      ...(worksOn ? { works_on: worksOn } : {}),
      ...(home ? { home } : {}),
      ...(filesOn ? { files_on: filesOn } : {}),
    });
    void store.refresh();
    return to;
  },
  opened(address) {
    location.hash = talkLink(address);
  },
  allowClaude: (machine, go, home) => consent.allow(machine, go, home),
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

const specimenSaid = english ? "this is a specimen" : "这是样例，没有连着世界";

const projectForm = new ProjectForm({
  store,
  words: english ? enProjectForm : zhProjectForm,
  create(town, fields, login) {
    if (!live) return Promise.reject(new Error(specimenSaid));
    return projectNew(town, fields, login?.account ?? null, login?.hubName ?? null);
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

const notLive = () => Promise.reject(new Error(specimenSaid));
/** A specimen computer's addresses: one on its network, one on a VPN. */
const specimenAddresses = [
  { interface: "Ethernet", address: "192.168.1.20", physical: true },
  { interface: "Wi-Fi", address: "10.0.0.7", physical: true },
  { interface: "Meta", address: "198.18.0.1", physical: false },
];
/** Makes a change as a login, and asks the World again. */
const change = async (login: Login, made: AccountChange): Promise<void> => {
  if (!live) {
    const changed = specimenChange(store.world.peek(), login.hub, made);
    if (!changed) return notLive();
    store.world.set(changed);
    return;
  }
  await accountChange(login.hubName, login.account, made);
  void store.refresh();
};
const accountConfirm = new Confirm(english ? enAccess : zhAccess);
const tokens = new Tokens({
  words: english ? enToken : zhToken,
  addresses: (login) => store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub)?.addresses ?? (live ? [] : ["192.168.1.20:7443"]),
  // A specimen shows what a token looks like; it lets nobody in.
  token: (login, kind, register, name, hours) =>
    live
      ? accountChange(login.hubName, login.account, {
          change: "token",
          register,
          name,
          kind,
          expires: new Date(Date.now() + hours * 3_600_000).toISOString(),
        }).then((token) => token ?? Promise.reject(new Error("the hub made no token")))
      : Promise.resolve(`FX5${"0SPECIMEN0NOT0A0REAL0TOKEN".repeat(4)}${hours}`),
  server: (login, name, hours) =>
    live ? serverCommand(login.hubName, login.account, name, hours) : Promise.resolve("echo 'Specimen: no real token or download'"),
  when: (at) => when(at, new Date()),
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
  newProject: (login) => projectForm.open(null, login),
});
document.getElementById("app")!.append(shell.element);

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
  return { here: world.here, machines: world.who.machines, others: world.who.others ?? [], nodes: machinesOf(world), towns: world.towns, away };
}

/** The parts of the World's present whose change leaves something behind
 *  to tell: a turn ending, a job going, a machine coming or going, a
 *  journal growing, someone arriving. */
function shapeOf(world: WorldNow): string {
  return JSON.stringify([
    residentsOf(world).map((r) => [r.about.id, r.about.machine, r.now?.activity ?? null, r.now?.turn ?? null, r.town]),
    jobsOf(world).map((job) => job.job),
    machinesOf(world).map((machine) => [machine.written, machine.online]),
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
      logins: loginsOn,
      worksOn: {
        places: () => worksPlacesFor(address),
        set: (worksOn) => (live ? setWorksOn(address, worksOn) : Promise.reject(new Error(specimenSaid))),
      },
      lentBy: (r) => {
        const lent = borrowedBy(store.world.peek(), r);
        return lent ? (lent.owner ?? lent.machine) : null;
      },
      actingFor: (r) => {
        const acting = r.about.acting;
        if (!acting || acting.kind === "here") return [];
        const logins = loginsOf(store.world.peek()?.accounts);
        return acting.accounts.map((one) => {
          const known = logins.find((login) => sameLogin(login, one));
          return known ? loginLabel(known) : one.account;
        });
      },
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

/** The machine the agent at `address` lives on, where an account it works
 *  for may work on it (not so for an agent lent to the user), and the login
 *  it works for among this computer's, as this computer reaches them. */
function livesFor(address: Address): { lives: Machine | null; login: Login | null } {
  const world = store.world.peek();
  const lives = machinesOf(world).find((machine) => machine.name === address.machine) ?? null;
  const acting = residentAt(world, address)?.about.acting;
  const logins = loginsOf(world?.accounts);
  const login =
    acting?.kind === "accounts" ? (logins.find((one) => acting.accounts.some((a) => sameLogin(one, a))) ?? null) : (logins[0] ?? null);
  return { lives, login };
}

/** Where the agent at `address` may work: machines an account it works for
 *  may work on. */
function worksPlacesFor(address: Address): string[] {
  const found = livesFor(address);
  return found.lives ? workPlaces(store.world.peek(), found.lives, found.login).map((machine) => machine.written) : [];
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
  // The drawing of the World; a machine on it opens on the machines page,
  // a hub on the account page.
  const drawing = new NetworkMap((key) => {
    location.hash = key.startsWith("hub:") ? link({ kind: "account" }) : link({ kind: "machine", name: key.replace(/^(m|lent):/, "") });
  });
  const mapWords = english ? enMachines : zhMachines;
  const page = new HomePage({
    drawing: drawing.element,
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
    if (world) {
      page.update(stateOf(world, away), now);
      const map = mapOf(world, mapWords);
      drawing.update(map.hubs, map.nodes, null, map.hubLabels);
    }
    if (live && world) {
      const next = shapeOf(world);
      if (next !== shape) {
        shape = next;
        void ask();
      }
    }
    if (trouble) page.unreachable(trouble, now, oldWorld.get());
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
      project: sample.town,
    },
    head,
    link: talkLink,
    conversation,
    opening: { open: notLive, reveal: notLive },
    pictures: {
      thumbnail: async (_machine, path, max) => specimenPicture(path, max),
      attachmentThumbnail: async (attachment, max) => specimenPicture(attachment.name, max),
      original: async (_machine, path) => (await fetch(specimenPicture(path, 2400))).blob(),
    },
    controls: {
      words: composerWords,
      offers: async () => (sample.agent === "claude" ? specimenClaudeOffers : specimenOffers),
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
    // The shared input kept in the page; `?queued` puts two in its queue.
    composer: () =>
      ComposerSession.open(address, {
        transport: specimenComposer(address, () => conversation.busy(), params.has("queued") ? (english ? ["Then run the overflow test on lab", "And write the result into the journal"] : ["然后在 lab 上跑溢出测试", "再把结果写进手记"]) : []),
      }),
    files: { words: composerWords.files, find: specimenFiles },
    details: detailsOf(address, async () =>
      known
        ? {
            resident: known,
            records: `/home/me/.fastctx/world/towns/${known.town ?? "outside"}/residents/${known.about.id}`,
            actions: known.last_action ? [known.last_action] : [],
            actions_total: known.last_action ? 41 : 0,
            asked: known.about.origin.kind === "colleague" ? known.about.origin.label : null,
            answers: [],
            jobs: [],
          }
        : null,
      exported(conversation, { agent: sample.agent === "claude" ? "Claude" : "Codex", machine: sample.machine, model: sample.model, project: sample.town }),
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
    stopJob: async () => {},
    jobOutput: async (job) => specimenOutput(job.command),
  });
  // What it started that still runs, as the specimen World lists it.
  const runningJobs = effect(() => {
    const now = store.clock.get();
    page.running(
      jobsOf(specimen.world).filter((job) => {
        const owner = ownerResident(job.owner);
        return owner?.resident === address.resident && owner.machine === address.machine;
      }),
      now,
    );
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
      runningJobs();
      page.leave();
    },
  };
}

/** Who a resident is, as the World last listed it, removed or not; null
 *  while it is not listed yet. */
function someoneOf(world: ReturnType<typeof store.world.peek>, address: Address): Someone | null {
  const known = residentAt(world, address);
  if (!known) return null;
  const lent = borrowedBy(world, known);
  return {
    agent: agentName(known.about.host.agent),
    machine: address.machine,
    worksOn: known.about.works_on && known.about.works_on !== address.machine ? known.about.works_on : null,
    lentBy: lent ? (lent.owner ?? lent.machine) : null,
    model: known.about.model ?? null,
    project: known.town ? (world?.towns.find((t) => t.town === known.town)?.record.name ?? known.town) : null,
  };
}

function liveTalk(address: Address): Page {
  const world = store.world.peek();
  const listed = someoneOf(world, address);
  const someone: Someone = listed ?? { agent: address.resident, machine: address.machine, model: null, project: null };
  const here = world?.here ?? null;
  const conversation = new Conversation();
  // Names of files for an `@`, on the agent's machine, in its places.
  const search = new FileSearch(address.machine, address.resident);
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
    composer: () => ComposerSession.open(address),
    // A file elsewhere is brought to this device first; one here opens
    // where it is.
    opening: {
      open: async (machine, path) => openLocal(await fetchFile(machine, path)),
      reveal: onComputer() ? reveal : undefined,
    },
    pictures: {
      thumbnail: async (machine, path, max) => thumbnailUrl(await thumbnail(machine, path, max)),
      attachmentThumbnail: async (attachment, max) => thumbnailUrl(await attachmentThumbnail(attachment, max)),
      original: async (machine, path) => {
        const read = await readBytes(machine, path);
        return new Blob([Uint8Array.from(atob(read.base64), (char) => char.charCodeAt(0))]);
      },
      reveal: onComputer() ? reveal : undefined,
    },
    files: {
      words: composerWords.files,
      find: (query) => search.find(query).then((files) => files.map((file) => ({ path: file.path, relative: file.relative }))),
      nowhere: () => {
        const now = store.world.peek();
        const known = residentsOf(now).find((r) => sameAddress(addressOf(r), address));
        if (!known) return null;
        if (!known.town) return "outside";
        const record = now?.towns.find((t) => t.town === known.town)?.record;
        return record && !record.places.some((place) => place.machine === address.machine) ? "unlisted" : null;
      },
    },
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
      filesOn: {
        places: () => {
          const known = residentAt(store.world.peek(), address);
          const lent = known ? borrowedBy(store.world.peek(), known) : null;
          return lent
            ? filePlaces(store.world.peek(), lent.login).map((machine) => ({
                value: machine.written,
                label: machine.written,
                note: machine.here ? (english ? "this one" : "这台") : machine.online === false ? (english ? "offline" : "离线") : undefined,
                disabled: !machine.here && machine.online === false,
              }))
            : null;
        },
        usual: () => {
          try {
            return localStorage.getItem("files-on");
          } catch {
            return null;
          }
        },
        label: (english ? enNewTalk : zhNewTalk).filesOn,
        what: (english ? enNewTalk : zhNewTalk).filesOnWhat,
      },
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
      return talk(address, text, attached.refs as Attachment[], attached.filesOn ?? null)
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
    stopJob: (job, machine) => stopJob(job, machine),
    jobOutput: (job) => RemoteLines.open(job.machine, job.output, { kind: "output", job: job.job }),
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
    switch (piece.kind) {
      case "event":
        page.resumed();
        conversation.event(piece.event);
        return;
      case "reset":
        page.resumed();
        conversation.history(piece.history);
        return;
      case "interrupted":
        page.broken(piece.error);
        return;
      case "ended":
        page.lost(piece.error);
    }
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
  // Opened before the World first answered, as from a notification at
  // start: built again once it has, to name the agent and its project.
  // Otherwise the World's listing of it is followed in place, without
  // reading the conversation again: named once listed, as one just started;
  // its project and model as they change; asleep or not.
  let shown = listed;
  const named = !world
    ? effect(() => {
        if (!store.world.get()) return;
        queueMicrotask(() => {
          if (!left && place.kind === "talk" && place.address.resident === address.resident && place.address.machine === address.machine) show(place);
        });
      })
    : effect(() => {
        const now = store.world.get();
        const known = residentAt(now, address);
        const who = someoneOf(now, address);
        if (!known || !who || left) return;
        page.presence(known.removed ? "removed" : null);
        if (shown && who.agent === shown.agent && who.project === shown.project && who.model === shown.model && who.worksOn === shown.worksOn) return;
        shown = who;
        page.identify(who);
      });
  return {
    element: page.element,
    title: `${someone.agent} · ${someone.machine}`,
    leave() {
      left = true;
      named();
      running();
      page.leave();
      search.close();
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
    link: talkLink,
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
        access: english ? enAccess : zhAccess,
        head,
        focus: next.kind === "machine" ? next.name : null,
        when,
        change,
        invite: (login) => tokens.open(login),
        talk: (machine) => newTalk.open(null, null, machine),
        confirm: accountConfirm,
      });
      break;
    case "agents":
      page = new AgentsPage({
        store,
        words: english ? enAgents : zhAgents,
        access: english ? enAccess : zhAccess,
        homeWords: agentHomeWords,
        head,
        machine: next.machine,
        when,
        agents: () => agentsOn(null),
        applyHere: (agent, into, home, raise = false) => (live ? applyHosts(agent === "codex", agent === "claude", raise, into, home) : notLive()),
        consent: {
          allow: (machine, home) => consent.allow(machine, (english ? enConsent : zhConsent).allow, home),
          withdraw: (machine, home) => consent.withdraw(machine, home),
        },
        homes: (machine, change) => (live ? agentHomes(machine, change) : notLive()),
        change,
        talkWith: (key) => newTalk.open(null, key),
        confirm: accountConfirm,
      });
      break;
    case "talks":
      page = new TalksPage({ store, words: english ? enTalks : zhTalks, shell: shellWords, head, town: next.town, place: () => place });
      break;
    case "features":
    case "feature":
      page = new FeaturesPage({ store, words: english ? enFeatures : zhFeatures, homeWords: agentHomeWords, english, head, feature: next.kind === "feature" ? next.feature : null, agents: () => agentsOn(null) });
      break;
    case "login": {
      // The World's drawing, for the moment this computer is in it.
      const loginMap = new NetworkMap(() => {});
      let marked: string | null = null;
      const drawLoginMap = () => {
        const world = store.world.peek();
        if (!world) return;
        const map = mapOf(world, english ? enMachines : zhMachines);
        loginMap.update(map.hubs, map.nodes, marked ? `m:${marked}` : null, map.hubLabels);
      };
      const stopLoginMap = effect(() => {
        store.world.get();
        drawLoginMap();
      });
      page = new LoginPage({
        store,
        words: english ? enLogin : zhLogin,
        head,
        device: asDevice,
        setup: asDevice ? undefined : () => (live ? setupState() : Promise.reject(new Error("specimen"))),
        place: asDevice ? undefined : () => (live ? placeFastctx() : notLive()),
        login: (hub, account, password, register, name) => (live ? login(hub, account, password, register, name) : notLive()),
        loginToken: (token, name) => (live ? loginToken(token, name) : notLive()),
        openAccount: asDevice ? undefined : (hub, account, password, register, name) => (live ? openAccount(hub, account, password, register, name) : notLive()),
        addresses: asDevice ? undefined : () => (live ? hereAddresses() : Promise.resolve(specimenAddresses)),
        // A specimen pretends, so the page after it can be looked at.
        create: asDevice
          ? undefined
          : (world, admin, password, reach, register, name) =>
              live ? createWorld(world, admin, password, reach, register, name) : Promise.resolve({ hub: world, name: name ?? "desktop", account: admin, kept: { kind: "installed" as const } }),
        invite: (login) => tokens.open(login),
        // A phone reads the token off the computer's screen; a specimen
        // shows the button when asked as a phone.
        scan: canScan() ? scanToken : !live && params.has("phone") ? async () => "FX5SPECIMEN" : undefined,
        keep: asDevice ? undefined : () => (live ? keepHere() : notLive()),
        drawing: {
          element: loginMap.element,
          mark: (machine) => {
            marked = machine;
            drawLoginMap();
          },
          stop: stopLoginMap,
        },
        enter() {
          welcomed.now();
          location.hash = link({ kind: "home" });
        },
      });
      break;
    }
    case "account":
      page = new AccountPage({
        store,
        words: english ? enAccount : zhAccount,
        machines: english ? enMachines : zhMachines,
        head,
        when,
        change,
        invite: (login, kind) => tokens.open(login, kind),
        confirm: accountConfirm,
      });
      break;
    case "admin":
      page = new AdminPage({
        store,
        words: english ? enAdmin : zhAdmin,
        head,
        when,
        change,
        confirm: accountConfirm,
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
        readBytes: (machine, path) => (live ? readBytes(machine, path) : notLive()),
        ago: (at) => homeWords.ago(Date.now() - at.getTime()),
      });
      break;
    case "settings":
      page = new SettingsPage({
        store,
        words: english ? enSettings : zhSettings,
        removalWords: english ? enRemoval : zhRemoval,
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
        phone: live ? (onPhone() ? phoneSeam : undefined) : params.has("phone") ? specimenPhone : undefined,
        notifyChanged: (on) => {
          if (live && onPhone()) void configurePhone(presenceWords, on).catch((error: unknown) => console.error("telling the phone its calls", error));
        },
        computer: live
          ? onComputer()
            ? {
                setup: setupState,
                keep: keepHere,
                removing: { plan: removalPlan, run: removeHere, finish: removeApp },
                retention: { read: retentionRead, write: retentionWrite },
                renewing: {
                  state: renewalState,
                  // The program the interface carries is put in place,
                  // then the node is asked to hand over to it.
                  renew: async () => {
                    await placeFastctx();
                    await renewHere();
                  },
                  guardian: renewGuardianHere,
                },
                updating,
              }
            : undefined
          : specimenComputer(),
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
      return { kind: "account" };
    case "welcome":
    case "login":
      return { kind: "login" };
    case "account":
      return { kind: "account" };
    case "admin":
      return { kind: "admin" };
    case "settings":
      return { kind: "settings" };
    case "codex":
      return { kind: "talk", address: { resident: "codex-7f3a", machine: "laptop" } };
    case "asks":
      return { kind: "talk", address: { resident: "codex-a51c", machine: "laptop" } };
    case "decision":
      return { kind: "file", machine: "desktop", path: "D:/work/mark50/specs/控制环.md", tail: false };
    case "log":
      return { kind: "file", machine: "lab", path: "/home/me/.fastctx/world/towns/mark50/residents/codex-3c77/out/j-p0q1r2.log", tail: true };
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
      ...loginsOf(world?.accounts).map((login) => ({
        group: paletteWords.actions,
        label: paletteWords.invite(loginLabel(login)),
        run: () => tokens.open(login),
      })),
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
      ...(loginsOf(world?.accounts).length ? [{ group: paletteWords.places, label: shellWords.account, run: go({ kind: "account" }) }] : []),
      { group: paletteWords.places, label: shellWords.logIn, run: go({ kind: "login" }) },
      ...residentsOf(world)
        .filter((r) => r.here || r.now?.waiting_for_user)
        .map((r) => ({
          group: paletteWords.agents,
          label: titleOf(r) ?? shellWords.someone(agentName(r.about.host.agent), r.about.machine),
          note: [titleOf(r) ? shellWords.someone(agentName(r.about.host.agent), r.about.machine) : null, r.town].filter(Boolean).join(" · "),
          run: go({ kind: "talk", address: { resident: r.about.id, machine: r.about.machine } }),
        })),
      ...machinesOf(world).map((machine) => ({
        group: paletteWords.machines,
        label: machine.written,
        note: machine.here || machine.online ? "" : shellWords.offline,
        run: go({ kind: "machine", name: machine.name }),
      })),
    ];
    return items;
  },
});

/** Whether this computer has been shown the way to log in: on its own it
 *  is shown it once, by itself, and after that from the sidebar. */
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
    if (!loginsOf(world.accounts).length && !welcomed.read() && place.kind === "home") location.hash = link({ kind: "login" });
  });
  // A computer the World cannot be asked on at all, before any answer: when
  // fastctx is not on it yet, the way on is to put it there.
  effect(() => {
    const trouble = store.trouble.get();
    if (!trouble || decided || store.world.peek()) return;
    decided = true;
    void setupState()
      .then((setup) => {
        if (!setup.installed && place.kind === "home") location.hash = link({ kind: "login" });
      })
      .catch(() => {});
  });
}

installPens();
installHints();
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
// A device logged in nowhere has nothing to show but the way in.
if (live) {
  void connectionState().then((state) => {
    asDevice = state.mode === "device";
    if (asDevice && !state.device?.hubs.length && place.kind === "home") location.hash = link({ kind: "login" });
    else if (place.kind === "login") show(place);
  });
}
if (!live && params.get("s") === "new") newTalk.open();
if (!live && params.get("s") === "new-project") projectForm.open();
if (!live && params.get("s") === "invite") {
  const [first] = groupsOf(specimen.world);
  if (first) tokens.open(first.login);
}

// On a phone the app's own process calls the user, from the rules and
// words the page gives it; a tap on a call opens what it was about.
if (live && onPhone()) {
  void configurePhone(presenceWords, notifying()).catch((error: unknown) => console.error("telling the phone its calls", error));
  void routeActivations().catch((error: unknown) => console.error("opening tapped calls", error));
}

startPresence({
  store,
  words: presenceWords,
  someone: (r) => shellWords.someone(agentName(r.about.host.agent), r.about.machine),
  stop: (address) => stop(address),
  say: (address, text) => talk(address, text),
});
