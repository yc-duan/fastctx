// The interface's entry: the frame with its sidebar, and the place the
// address names in it (`app/places.ts`). Inside the shell it reads the
// World on this machine; in a plain browser a specimen stands in, and
// `?s=home|claude|codex|project|journal|machines|settings|new`,
// `?theme=…`, `?lang=en` and `?live` pick what to look at.

import { focusPlace } from "./app/focus-places";
import { followSwitches } from "./app/segment-slide";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { Attachment, FilePlace, UploadTarget } from "./world/attachments";
import type { AssistantFiles } from "./conversation/node-tray";
import { standsUnder } from "./conversation/landing";
import "katex/dist/katex.min.css";
import "./design/fonts.css";
import "./design/tokens.css";
import "./design/breath.css";
import "./design/base.css";
import "./design/highlight.css";
import "./conversation/conversation.css";
import "./conversation/turns.css";
import "./conversation/model-panel.css";
import "./conversation/agents.css";
import "./longview/longview.css";
import "./app/shell.css";
import "./app/press.css";
import "./app/sheet.css";
import "./app/updates.css";
import "./app/versions.css";
import "./app/pages.css";
import "./app/accounts.css";
import "./app/me.css";
import "./app/when.css";
import "./app/assist.css";
import "./app/machines.css";
import "./app/machine-framed.css";
import "./app/machine-list.css";
import "./app/machine-spot.css";
import "./app/machine-find.css";
import "./app/update-view.css";
import "./app/machine-page.css";
import "./app/machine-manage.css";
import "./app/agents.css";
import "./app/features.css";
import "./app/removal.css";
import "./app/managed.css";
import "./app/progress-steps.css";
import "./app/data-page.css";
import "./app.css";
import "./conversation/talk-usage.css";
import "./conversation/context-sheet.css";
import "./home/home.css";
import "./home/front.css";
import "./project/project.css";
import "./pages/pages.css";

import { MePage } from "./app/me";
import { ProjectActs, enProjectActs, zhProjectActs } from "./app/project-acts";
import { enMe, zhMe } from "./app/me-words";
import { enPeople, zhPeople } from "./app/people-words";
import { LoginPage } from "./app/login";
import { MachinesPage } from "./app/machines";
import { AgentLoginsPage } from "./app/agent-logins";
import { LoginUsagePage } from "./app/usage/login-page";
import { enUsage, zhUsage } from "./app/usage/words";
import "./app/usage/usage.css";
import { enCalls, zhCalls } from "./app/usage/calls-words";
import "./app/usage/calls.css";
import { keptRecord } from "./app/usage/kept";
import { ActsMenu } from "./app/acts-menu";
import { specimenUsage } from "./specimen/usage";
import { specimenCalls } from "./specimen/calls";
import { enAgentLogins, zhAgentLogins } from "./app/agent-logins-words";
import { enProfile, zhProfile } from "./app/profile-words";
import { isHost, keepSection, sectionOf, tabOf } from "./app/agent-sections";
import "./app/agent-sections.css";
import { PromptPage } from "./app/prompt-page";
import { HostPrefs } from "./app/host-prefs";
import { SubagentsPage } from "./app/subagents-page";
import { profileSubagents } from "./world/subagents";
import { specimenSubagents } from "./specimen/subagents";
import { ProfileDefaults } from "./app/profile-defaults";
import { McpPage } from "./app/mcp-page";
import type { ProfileCalls } from "./app/profile-account";
import "./app/profile.css";
import "./app/host-prefs.css";
import "./app/subagents.css";
import { mcp as mcpCall, readProfile, writeProfile } from "./world/profile";
import { specimenProfileCalls } from "./specimen/profile";
import { ServerLists } from "./app/mcp-switches";
import type { FocusServers } from "./app/focus-panel";
import { enAgentChange, zhAgentChange } from "./app/agent-change";
import "./app/agent-logins.css";
import { specimenChange as specimenLoginChange, specimenFolder, specimenLogins, specimenPreview, specimenScan, specimenSetTitle, specimenTakeUp } from "./specimen/logins";
import { TalksPage, enTalks, zhTalks } from "./app/talks";
import { FeaturesPage } from "./app/features";
import { deviceFeatures } from "./app/device-features";
import { readFeatures, setFeature, setLanguage, watchFeatures } from "./world/features";
import { untilAnswered, whyOf } from "./world/again";
import { enFeatures, zhFeatures } from "./app/features-words";
import { enHomes, zhHomes } from "./app/agent-homes";
import { Tokens, type CodeAsked } from "./app/tokens";
import { keepLanguage } from "./language";
import { codeName, codesOf, enCodes, zhCodes } from "./app/code-rows";
import { ManagedPage, managedOf } from "./app/managed-page";
import { enManaged, zhManaged } from "./app/managed-words";
import { RemovalSheet } from "./app/removal-sheet";
import { noticeDissolved } from "./app/dissolved-notice";
import { NewTalk } from "./app/new-talk";
import { enModelPanel, zhModelPanel } from "./conversation/model-panel";
import "./app/new-talk.css";
import "./app/talk-menu.css";
import "./app/focus-panel.css";
import "./app/project-form.css";
import "./app/folder-picker.css";
import "./app/fingers.css";
import { Palette, type PaletteItem } from "./app/palette";
import { ProjectForm } from "./app/project-form";
import { fileLink, link, placeOf, talkLink, type Place } from "./app/places";
import { MachinePage } from "./app/machine-page";
import { enMachinePage, zhMachinePage } from "./app/machine-page-words";
import { enManage, zhManage } from "./app/machine-manage-words";
import { enBatch, zhBatch } from "./app/machine-batch-words";
import { enNameSaid, zhNameSaid } from "./app/name-words";
import { specimenStats } from "./specimen/stats";
import { SettingsPage } from "./app/settings";
import { DataPage, enData, totalOf, zhData } from "./app/data-page";
import { data } from "./world/data";
import { keptUsage, keptClear, seenDissolved, unwatch } from "./world/bridge";
import { specimenCache, specimenData } from "./specimen/data";
import { ReaderPage } from "./app/reader";
import { Shell, pageHead, type Page } from "./app/shell";
import { effect, signal } from "./app/signals";
import { addressOf, createStore, isUnreached, jobsOf, residentAt, residentsOf, sameAddress, unreachedOf } from "./app/store";
import { TalkActs } from "./app/talk-acts";
import { TalkMenu } from "./app/talk-menu";
import { enTalkMenu, zhTalkMenu } from "./app/talk-menu-words";
import { TalkToast } from "./app/talk-toast";
import type { Controlled, PendingControl } from "./world/control";
import type { JsonValue } from "./world/wire";
import { escape as escapeText } from "./conversation/prose";
import {
  enComposer,
  enConsent,
  enPresence,
  enDetails,
  enFocus,
  enNewTalk,
  enPalette,
  enProjectForm,
  enSettings,
  enShell,
  zhComposer,
  zhConsent,
  zhPresence,
  zhDetails,
  zhFocus,
  zhNewTalk,
  zhPalette,
  zhProjectForm,
  zhSettings,
  zhShell,
  readerWords } from "./app/words";
import { enRemoval, zhRemoval } from "./app/removal-words";
import { enAccess, enLogin, enMachines, enToken, zhAccess, zhLogin, zhMachines, zhToken } from "./app/account-words";
import { LASTING, loginsOf, type AccountChange, type Login, type TokenSeen, type TokenWay } from "./world/accounts";
import { assistantOf, assistantsOf, assistingOwner, fileHome, filePlaces, groupsOf, inNoNetwork, loginKey, loginLabel, machinesOf, residentIn, sameLogin, workPlaces, type Machine } from "./app/reach";
import type { FocusPlace } from "./app/focus-panel";
import { folders as foldersOf, noFocus, type Focus, type Folders } from "./world/focus";
import { ComposerSession } from "./conversation/composer";
import { allowPrivacy, attachmentThumbnail, fetchAttachment, fetchFile, openLocal, reveal, saveAs, thumbnail, thumbnailUrl, type Saved, type Thumbnail } from "./world/files";
import { specimenComposer, type SpecimenComposer } from "./specimen/composer";
import { ClaudeConsent } from "./app/claude-consent";
import { Conversation } from "./conversation/conversation";
import { LocalTranscript } from "./specimen/transcript";
import { ConversationPage, type Someone } from "./conversation/page";
import { KeptPages, type KeepablePage } from "./conversation/kept";
import type { Seed } from "./conversation/starting";
import { watched } from "./conversation/source";
import { redrawDiagrams } from "./conversation/diagrams";
import { HomePage } from "./home/page";
import type { Away, WorldState } from "./home/model";
import { happeningsOf } from "./home/since";
import type { SinceEvent } from "./world/since";
import { enHome, zhHome } from "./home/words";
import { installHints } from "./app/hint";
import { installSheetDrag } from "./app/sheet-drag";
import { installPresses } from "./app/press";
import { installPointerHints } from "./app/pointer-hint";
import { installPens } from "./pen/filters";
import { installRules } from "./pen/rules";
import { MemoryPage } from "./project/memory-page";
import { enMemory, zhMemory } from "./project/memory-words";
import { memory as memorySeam, type MemoryAuthor } from "./world/memory";
import { specimenCatalog, specimenDocument } from "./specimen/memory";
import { sentinelList, sentinelListAll, sentinelStop, sentinels as liveSentinels, type SentinelSummary } from "./world/sentinels";
import { planRead } from "./world/plan";
import { sawTold, toldSeen } from "./app/told-seen";
import { specimenHubPlans, specimenHubSenses } from "./specimen/hub";
import type { Job as PanelJob } from "./conversation/jobs-panel";
import { specimenSentinels } from "./specimen/sentinels";
import { JournalForm } from "./project/journal-form";
import { ProjectPage } from "./project/page";
import { enProject, zhProject } from "./project/words";
import { specimenMap } from "./specimen/project";
import { markdownOf } from "./conversation/export";
import { specimens, type Specimen } from "./specimen/sessions";
import { answering, playLater, playTimed, streamSpecimen } from "./specimen/streams";
import { specimenAgentRecord } from "./specimen/agents";
import { specimenAgents, specimenAsManaged, specimenAsShared, specimenAsVisitor, specimenChange, specimenNow, specimenRenameHere } from "./specimen/world";
import { specimenHubMove } from "./specimen/hub-move";
import { specimenRemote } from "./specimen/remote";
import { specimenNameProblem } from "./specimen/names";
import { specimenMachineLog } from "./specimen/machine-log";
import type { RemoteSeams } from "./app/remote-world";
import { specimenClaudeOffers, specimenComputer, specimenFiles, specimenOffers, specimenOutput, specimenPeek, specimenPhone, specimenPicture, specimenRead, specimenUpload } from "./specimen/seams";
import { RemoteLines } from "./world/remote-lines";
import { specimenLines } from "./specimen/run-facts";
import { applyTheme, chooseTheme, currentTheme, themes } from "./theme";
import { Confirm } from "./app/confirm";
import { showPathNotice } from "./app/path-notice";
import { PageScreen } from "./pages/view";
import { forgetConversation, manage as manageResident, type Forgetting, type Managed, type Management } from "./world/management";
import { FileSearch } from "./world/file-search";
import { configurePhone, onPhone, phoneSeam, routeActivations } from "./app/phone";
import { renewalState, renewHere, type VersionMismatch } from "./world/renewal";
import { toolsFor } from "./pages/tools";
import { enPages, zhPages } from "./pages/words";
import { act as pageAct, consent as pageConsent, pageHistory, readPage, restore as restorePage, withdraw as withdrawPage } from "./world/pages";
import { specimenPageTools, specimenPageWorld } from "./specimen/pages";
import type { Address, SessionEvent } from "./world/events";
import type { MachineLogins } from "./world/logins";
import { ownerResident, type ResidentNow, type WorldWho } from "./world/world";
import { agentName, titleOf } from "./world/names";
import { liveSide, specimenSide } from "./side/live";
import { enSide, zhSide } from "./side/words";
import { enPlan, zhPlan } from "./plan/words";
import { samples as planSamples } from "./plan/specimen-samples";
import { MachineMap, MAP_ORDER, type MachineMapScene } from "./app/machine-map";
import { mapOf } from "./app/map-of";
import {
  addAgentLogin,
  agentFolder,
  agentCalls,
  agentLogins,
  agentUsage,
  changeAgentLogin,
  pickFolder,
  previewAgentChange,
  readAgentLimits,
  setAgentTitle,
  scanAgents,
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
  remoteClose,
  remoteHub,
  arriveHub,
  hubMove,
  remoteArrive,
  remoteJoin,
  hubProxy,
  remoteLook,
  remoteOpen,
  remoteProbe,
  remoteWithdraw,
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
  retryNow,
  type WorldNow,
  login,
  loginToken,
  peekToken,
  nameProblem,
  openAccount,
  createWorld,
  hereAddresses,
  accountChange,
  serverCommand,
  machineLog,
  machineStats,
  renameHere,
  agentActivity,
} from "./world/bridge";
import { en, zh } from "./words";
import { updates as updateSeam } from "./world/updates";
import { Updates } from "./app/updates";
import { UpdateCard } from "./app/update-card";
import { VersionWindow } from "./app/version-window";
import { showAttachedWith } from "./conversation/attached-pictures";
import { viewPictures } from "./conversation/pictures";
import { enUpdate, zhUpdate } from "./app/update-words";
import { specimenUpdates } from "./specimen/updates";
import { specimenUpdateView } from "./specimen/update-view";
import { networkUpdates } from "./world/update-view";
import { enUpdateView, zhUpdateView } from "./app/update-view-words";
import { getVersion } from "@tauri-apps/api/app";
import { desktopAvailable } from "./world/desktop";
import { answerWith, notifying, startPresence } from "./app/presence";
import { installAccess } from "./app/access";
import { installBack, setPageBack } from "./app/back";
import { installFrame } from "./app/frame";
import { nameCodeCopy } from "./conversation/prose";
import { agentKind } from "./app/model-memory";
import type { AgentModel } from "./world/agents";

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
const sideWords = english ? enSide : zhSide;
const planWords = english ? enPlan : zhPlan;
const pageWords = english ? enPages : zhPages;
const presenceWords = english ? enPresence : zhPresence;
const shellWords = english ? enShell : zhShell;
document.documentElement.lang = english ? "en" : "zh-CN";

const specimen = specimenNow(params.get("accounts") === "2");
// `?managed`: this computer as a managed machine; `?managed=removed`, one
// its owner took off the network.
if (params.has("managed")) specimen.world = specimenAsManaged(specimen.world, params.get("managed") === "removed");
// `?shared`: bo's gpu shared whole with this computer's account and ada's.
if (params.has("shared")) specimen.world = specimenAsShared(specimen.world);
// `?visitor`: this computer's account a visitor asked to become a member.
if (params.has("visitor")) specimen.world = specimenAsVisitor(specimen.world);
// `?accounts=0`: a computer in no World yet, as on its first open.
if (params.get("accounts") === "0") {
  specimen.world.accounts.hubs = [];
  specimen.world.nodes.hubs = [];
}
// `?dissolved`: a network this computer was on, dissolved by its
// administrator while it was away, said once.
if (params.has("dissolved")) specimen.world.accounts.dissolved = [{ hub: "lab", at: new Date().toISOString() }];
// `?quiet`: a quiet morning, nothing waiting, nothing come back, nobody at
// work.
if (params.has("quiet")) {
  for (const machine of specimen.world.who.machines) {
    if (machine.kind !== "answered") continue;
    machine.reply.residents = machine.reply.residents.map((r) => (r.now ? { ...r, now: { ...r.now, activity: "idle", waiting_for_user: false, turn: null } } : r));
    machine.reply.residents.forEach((r) => {
      if (r.now) delete r.now.asked;
    });
    machine.reply.jobs = [];
    delete machine.reply.busy;
  }
  if (specimen.away) specimen.away.happenings = [];
}
const store = createStore(() => ({ world: specimen.world, now: specimen.now }));
const live = store.live;
// A specimen in a development build lends its World and store to the page,
// so a probe can change the World and time what the interface does with it.
if (!live && import.meta.env.DEV) Object.assign(window, { __specimen: specimen, __store: store });
/** Whether this interface reaches the World as a device (a phone, or a
 *  computer started with --device): it joins as one and sets nothing up.
 *  A specimen shows one when asked. The shell's own answer follows. */
let asDevice = live ? !onComputer() : params.has("phone");

const head = (title: string, actions?: string) => pageHead(shellWords, title, actions, backName);
/** The account's own configuration, from the World or the specimen's
 *  memory, which lasts the visit. */
const profileWrites: ProfileCalls = live
  ? { read: readProfile, write: writeProfile, mcp: mcpCall }
  : specimenProfileCalls();
/** What the account's preferences name for a new conversation; whatever
 *  this device writes reaches it at once. */
const profileDefaults = new ProfileDefaults(profileWrites);
const profileCalls: ProfileCalls = {
  ...profileWrites,
  write: async (hub, account, profile) => {
    const version = await profileWrites.write(hub, account, profile);
    profileDefaults.saved(hub, account, { ...profile, version });
    return version;
  },
};
const serverLists = new ServerLists(profileCalls, store, english ? enProfile : zhProfile);
/** The account's Claude subagents: the World's, or the specimen's. */
const subagentCalls = live ? profileSubagents : specimenSubagents(params.get("subagents") === "older");

/** The models an agent's logins offer across the machines this computer
 *  reaches, each once, in the order the first machine to list it gave. */
async function agentModels(agent: "claude" | "codex"): Promise<AgentModel[]> {
  const world = await agentsOn(null);
  const seen = new Map<string, AgentModel>();
  for (const machine of world.machines) {
    if (machine.kind !== "answered") continue;
    for (const facts of machine.reply.agents) {
      if (agentKind(facts.agent) !== agent || facts.models.kind !== "known") continue;
      for (const model of facts.models.value) if (!seen.has(model.model)) seen.set(model.model, model);
    }
  }
  return [...seen.values()];
}

let place: Place = { kind: "home" };

/** The pages one comes into from another, which have a way back in their
 *  head (`world-gui.md`, 界面的结构); the sidebar's own pages have none. */
function inner(at: Place): boolean {
  return ["feature", "data", "memory", "page", "file", "login", "talks", "archived", "machine", "usage", "subagents"].includes(at.kind);
}

/** Where a page's back goes when it was not come into from another page,
 *  as when the window opened on it. */
function parentOf(at: Place): Place {
  switch (at.kind) {
    case "usage":
      return { kind: "agents", machine: at.machine };
    case "subagents":
      return { kind: "agents", machine: null, section: "claude", tab: "prefs" };
    case "feature":
      return { kind: "features" };
    case "data":
      return { kind: "settings" };
    case "machine":
      return { kind: "machines" };
    case "memory":
    case "page":
      return { kind: "project", town: at.town };
    case "archived":
      return { kind: "talks", town: null };
    default:
      return { kind: "home" };
  }
}

/** A place's name where its page cannot be asked. */
function placeName(at: Place): string {
  switch (at.kind) {
    case "home":
      return deviceFeatures.conversations.peek() ? shellWords.world : shellWords.agents;
    case "agents":
      return at.section === "claude" || at.section === "codex" ? (english ? enProfile : zhProfile).sections[at.section] : shellWords.agents;
    case "usage":
      return shellWords.agents;
    case "machines":
    case "machine":
      return shellWords.machines;
    case "features":
      return shellWords.features;
    case "settings":
      return shellWords.settings;
    case "account":
      return shellWords.account;
    case "project":
      return at.town;
    case "talks":
      return at.town ?? (english ? enTalks : zhTalks).title;
    default:
      return "";
  }
}

/** The pages the user came through into the one shown, the last first back:
 *  each was left by going to the next, so the window's history holds them in
 *  the same order. */
let trail: { place: Place; title: string }[] = [];
/** The page shown, whose name the trail keeps as it is left. */
let shownPage: Page | null = null;
/** The name of the way back of the page being made, read by `head`. */
let backName: string | null = null;
/** The address being replaced to: a redirect replaces the page, and is not
 *  come back to. */
let replacedTo: string | null = null;

function replacePlace(to: string): void {
  replacedTo = to;
  location.replace(to);
}

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
    : [...new Set([...machinesOf(specimen.world).filter((one) => one.here || one.online).map((one) => one.name), ...assistantsOf(specimen.world).map((one) => one.machine)])];
  return Promise.resolve({ machine: specimen.world.here, machines: names.flatMap((name) => specimenAgents(name, specimen.now).machines) });
}

/** The new-conversation pages, one for each project and one outside any:
 *  each kept while the interface is open, so what was written there and
 *  the files attached wait for the user to come back. */
const newTalks = new Map<string, NewTalk>();
/** A login or machine the next new-conversation page is opened for. */
let newTalkFor: { pick: string | null; machine: string | null } | null = null;

function newTalkOf(town: string | null): NewTalk {
  const key = town ?? "";
  let talk = newTalks.get(key);
  if (!talk) {
    talk = makeNewTalk(town);
    newTalks.set(key, talk);
  }
  return talk;
}

/** Opens a new conversation's page, in `town` or outside any, for a login
 *  (`pick`) or a machine when one was chosen where it was asked for. */
function beginTalk(town: string | null = null, pick: string | null = null, machine: string | null = null): void {
  newTalkFor = pick || machine ? { pick, machine } : null;
  const there = link({ kind: "new", town });
  if (location.hash === there) show(placeOf(there));
  else location.hash = there;
}

const makeNewTalk = (town: string | null) => new NewTalk({
  store,
  town,
  head: (title) => head(title),
  words: english ? enNewTalk : zhNewTalk,
  focusWords,
  servers: (login) => serverLists.switches(login),
  defaults: profileDefaults,
  homeWords: agentHomeWords,
  modelWords: english ? enModelPanel : zhModelPanel,
  agents: (machine) => agentsOn(machine),
  folders: (machine, path) => (live ? foldersOf(machine, path) : specimenFolders(machine, path)),
  async begin({ agent, machine, town, text, model, effort, attachments, account, focus, home, filesOn, mcpOff }) {
    if (!live) return specimenBegin(params.get("start"));
    const { to } = await openAgent(agent, machine, town, text, model, effort, attachments, {
      ...(account ? { account } : {}),
      ...(focus.machines.length ? { focus } : {}),
      ...(home ? { home } : {}),
      ...(filesOn ? { files_on: filesOn } : {}),
      ...(mcpOff.length ? { mcp_off: mcpOff } : {}),
    });
    void store.refresh();
    return to;
  },
  opened(address, seed) {
    // The first words open with the conversation, as they were shown.
    seeds.set(talkKey(address), seed);
    replacePlace(talkLink(address));
  },
  starting: { words, here: () => store.world.peek()?.here ?? "" },
  allowClaude: (machine, go, home) => consent.allow(machine, go, home),
  attach: {
    words: (english ? enComposer : zhComposer).attach,
    async upload(machine, target, source, progress, place) {
      if (!live) return specimenUpload(machine, target, source, progress);
      if (source.kind === "path") return uploadPath(machine, target, source.path, progress, place);
      const done = await uploadBytes(machine, target, source.file, place);
      progress(source.file.size);
      return done;
    },
    ...(live ? { drops: windowDrops } : {}),
  },
});

const specimenSaid = english ? "this is a specimen" : "这是样例，没有连着世界";
/** A specimen conversation begun: `?start=open` opens one that starts
 *  working, `wait` never hears back, `fail` fails as a machine would say;
 *  otherwise it says it is a specimen. */
function specimenBegin(start: string | null): Promise<Address> {
  if (start === "wait") return new Promise(() => {});
  if (start === "open") return new Promise((resolve) => setTimeout(() => resolve({ resident: "stream-1", machine: "laptop" }), 1800));
  if (start === "fail") {
    return new Promise((_, reject) =>
      setTimeout(() => reject(new Error("on laptop: starting the session: Claude Code exited before it began: Not logged in · Please run /login")), 2400),
    );
  }
  return Promise.reject(new Error(specimenSaid));
}
/** First words of conversations just begun, by conversation, for their
 *  pages to open with. */
const seeds = new Map<string, Seed | null>();

const projectForm = new ProjectForm({
  store,
  words: english ? enProjectForm : zhProjectForm,
  focusWords: english ? enFocus : zhFocus,
  folders: (machine, path) => (live ? foldersOf(machine, path) : specimenFolders(machine, path)),
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
    // Kept in the specimen, so what was done stays as the page is read again.
    const changed = specimenChange(specimen.world, login.hub, made);
    if (!changed) return notLive();
    specimen.world = changed;
    await store.refresh();
    return;
  }
  await accountChange(login.hubName, login.account, made);
  void store.refresh();
};
/** The way to make a World on a server over SSH, from this computer. */
const remoteSeams: RemoteSeams = {
  probe: remoteProbe,
  look: remoteLook,
  hub: remoteHub,
  open: remoteOpen,
  join: remoteJoin,
  arrive: remoteArrive,
  withdraw: remoteWithdraw,
  close: remoteClose,
  logout: async (hub, account) => {
    await accountChange(hub, account, { change: "logout" });
  },
  login: (address, admin, password, name) => login(address, admin, password, true, name),
  nameProblem,
  token: async (hub, admin, machine) => {
    // An hour is the time the server has to use it: it does at once.
    const token = await accountChange(hub, admin, { change: "token", register: true, name: machine, kind: "computer", expires: new Date(Date.now() + 3_600_000).toISOString() });
    if (!token) throw new Error("the hub made no token");
    return token;
  },
};
const accountConfirm = new Confirm(english ? enAccess : zhAccess);
/** The specimen's server moving, kept across pages. */
const specimenMove = specimenHubMove();
/** When a code asked for now stops letting devices in, as the hub is told. */
const codeExpires = (asked: CodeAsked) => (asked.expires ? asked.expires.toISOString() : LASTING);
/** A specimen's code: listed as a real one would be, letting nobody in. */
const specimenCode = async (login: Login, way: TokenWay, kind: "phone" | "computer", asked: CodeAsked): Promise<string> => {
  const made: TokenSeen = {
    key: `specimen-${Date.now().toString(36)}`,
    account: login.account,
    ...(asked.label ? { label: asked.label } : {}),
    kind,
    ...(asked.managed ? { managed: true } : {}),
    uses: asked.uses,
    used: 0,
    expires: codeExpires(asked),
    way,
    ...(asked.name ? { name: asked.name } : {}),
    made_by: specimen.world.accounts.machine,
    made_at: new Date().toISOString(),
  };
  specimen.world = { ...specimen.world, accounts: { ...specimen.world.accounts, hubs: specimen.world.accounts.hubs.map((hub) => (hub.hub === login.hub ? { ...hub, tokens: [...(hub.tokens ?? []), made] } : hub)) } };
  await store.refresh();
  return `FX5${"0SPECIMEN0NOT0A0REAL0CODE".repeat(5)}`;
};
/** Asks once, then revokes a code; whether it was revoked. */
const revokeCode = (login: Login, token: TokenSeen): Promise<boolean> =>
  new Promise((resolve) => {
    const words = english ? enCodes : zhCodes;
    accountConfirm.ask({
      title: words.revokeTitle(codeName(token, words)),
      what: words.revokeWhat,
      go: words.revokeGo,
      act: () => change(login, { change: "revoke_token", token: token.key }),
      done: () => resolve(true),
    });
  });
const tokens = new Tokens({
  words: english ? enToken : zhToken,
  codeWords: english ? enCodes : zhCodes,
  addresses: (login) => store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub)?.addresses ?? (live ? [] : ["192.168.1.20:7443"]),
  code: (login, kind, asked) =>
    live
      ? accountChange(login.hubName, login.account, {
          change: "token",
          register: kind === "computer",
          name: asked.name,
          kind,
          expires: codeExpires(asked),
          ...(asked.managed ? { managed: true } : {}),
          ...(asked.uses === 1 ? {} : { uses: asked.uses }),
          ...(asked.label ? { label: asked.label } : {}),
          way: kind === "phone" ? "scan" : "paste",
        }).then((token) => token ?? Promise.reject(new Error("the hub made no token")))
      : specimenCode(login, kind === "phone" ? "scan" : "paste", kind, asked),
  server: (login, asked) =>
    live
      ? serverCommand(login.hubName, login.account, asked.name, codeExpires(asked), asked.managed, asked.uses === 1 ? null : asked.uses, asked.label, asked.windows ?? false, asked.mac ?? false)
      : specimenCode(login, "line", "computer", asked).then(() => "echo 'Specimen: no real token or download'"),
  codes: (login) => codesOf(store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub), login, new Date()),
  revoke: revokeCode,
  here: () => store.world.peek()?.accounts.machine ?? "",
  manages: (login) => !live || !!store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub)?.managed,
  when: (at) => when(at, new Date()),
  nameProblem: live ? nameProblem : specimenNameProblem,
  // Over SSH only from a computer: a phone carries no Linux program.
  remote:
    live && !onComputer()
      ? undefined
      : {
          seams: live ? remoteSeams : specimenRemote(),
          words: english ? enLogin : zhLogin,
          token: (login, machine, managed) =>
            live
              ? accountChange(login.hubName, login.account, { change: "token", register: true, name: machine, kind: "computer", expires: new Date(Date.now() + 3_600_000).toISOString(), ...(managed ? { managed: true } : {}), way: "ssh" }).then(
                  (token) => token ?? Promise.reject(new Error("the hub made no token")),
                )
              : Promise.resolve("FX5SPECIMEN"),
          added: () => void store.refresh(),
        },
});

/** Taking fastctx off this computer, from wherever it is asked for. */
const removal = (() => {
  const removing = live ? (onComputer() ? { plan: removalPlan, run: removeHere, finish: removeApp } : undefined) : specimenComputer().removing;
  return removing ? new RemovalSheet(english ? enRemoval : zhRemoval, removing) : undefined;
})();

const memoryWords = english ? enMemory : zhMemory;
const journalForm = new JournalForm({
  words: memoryWords,
  close: projectWords.close,
  write: async (town, name, text) => {
    if (!live) return notLive();
    await memorySeam.save(town, "journal", name, text, null);
  },
  written(town, name) {
    const there = link({ kind: "memory", town, name });
    if (location.hash === there) show(placeOf(there));
    else location.hash = there;
  },
});

/** Whether project memory is on for the account, as last read: the
 *  specimen's unless `?memory=off`. */
let memoryOn = !live && params.get("memory") !== "off";
if (live) void memorySeam.enabled(null, null).then((on) => (memoryOn = on), () => (memoryOn = false));
const sentinelSeam = live ? liveSentinels : specimenSentinels(params.has("sentinels"));
/** The machines the sentinels may live on, the ones online first; a
 *  phone is a device, not one of them. */
/** The machines a login's sentinels may live on: its own and those shared
 *  with it whole, never a managed one, which acts for no one (`world-senses.md`,
 *  哨兵住在哪). */
function sentinelHomes(hub: string, account: string): { name: string; form: "desktop" | "laptop" | "server" }[] {
  const world = live ? store.world.peek() : specimen.world;
  if (!world) return [];
  const seen = world.accounts?.hubs.find((one) => one.name === hub);
  const homes = seen
    ? new Set(
        seen.members
          .filter((member) => member.kind === "computer" && !member.managed)
          .filter((member) => member.logins.includes(account) || member.home === account || seen.shares.some((share) => share.machine === member.name && share.account === account))
          .map((member) => member.name),
      )
    : null;
  return machinesOf(world)
    .filter((machine) => !homes || homes.has(machine.name))
    .sort((a, b) => Number(b.online) - Number(a.online))
    .map((machine) => ({ name: machine.written, form: machine.facts.Ok?.form ?? "desktop" }));
}
const memorySwitch = {
  enabled: async (hub: string | null, account: string | null) => (live ? (memoryOn = await memorySeam.enabled(hub, account)) : memoryOn),
  set: async (on: boolean, hub: string | null, account: string | null) => {
    if (live) await memorySeam.setEnabled(on, hub, account);
    memoryOn = on;
  },
};

/** What the user does to conversations from their "⋯", and the line that
 *  says what was just done (`app/talk-acts.ts`). */
const talkMenuWords = english ? enTalkMenu : zhTalkMenu;
const projectActsWords = english ? enProjectActs : zhProjectActs;
const projectActs = new ProjectActs({
  words: projectActsWords,
  access: english ? enAccess : zhAccess,
  store,
  confirm: accountConfirm,
  change: (login, made) => change(login, made),
  conversations: (town) => {
    const world = store.world.peek();
    return [...residentsOf(world, true), ...unreachedOf(world, true)].filter((r) => r.town === town);
  },
  deleted: (town) => {
    // A page of the project deleted is left for the front page.
    const at = placeOf(location.hash);
    if ((at.kind === "project" || at.kind === "memory" || at.kind === "page") && at.town === town) replacePlace(link({ kind: "home" }));
  },
});
const talkToast = new TalkToast({ undo: talkMenuWords.undo }, () => shell.footBox());
const talkActs = new TalkActs({
  words: talkMenuWords,
  manage: (address, action) => (live ? manageResident(address, action) : specimenManaged(address, action)),
  rename: (address, name) => (live ? control(address, { do: "rename", name }) : specimenRenamed(address, name)),
  forget: (address) => (live ? forgetConversation(address) : specimenForgot(address)),
  say: (text, options) => talkToast.say(text, options),
  refresh: () => void store.refresh(),
});
const talkMenu = new TalkMenu({
  words: talkMenuWords,
  acts: talkActs,
  towns: townsFor,
  reachable: (r) => !isUnreached(r),
  someone: (r) => someoneSaid(r),
  place: (r) => placeSaid(r),
  askDelete: (r, then) => askForget(r, then),
});
effect(() => {
  const now = store.world.get();
  talkActs.settle((address) => listedAt(now, address));
});

/** A conversation as the World lists it, archived or not, as its machine
 *  last answered when it is away. */
function listedAt(world: WorldNow | null, address: Address): ResidentNow | null {
  return [...residentsOf(world, true), ...unreachedOf(world, true)].find((r) => r.about.id === address.resident && r.about.machine === address.machine) ?? null;
}

const forgetConfirm = new Confirm({ close: talkMenuWords.cancel, working: talkMenuWords.working, failed: talkMenuWords.failed });

/** Asks before a conversation is deleted everywhere: what goes, and that it
 *  is stopped first or deleted once its machine is back where so; `then`
 *  once it is asked. */
/** Who a conversation is with, where it has no name of its own: the agent
 *  on its machine, or for one assisting the user, whose agent, never the
 *  computer it runs on (`world-network.md`, 共享机器与智能体协助). */
function someoneSaid(r: ResidentNow, said: { someone(agent: string, machine: string): string; someoneAssisting(agent: string, owner: string): string } = shellWords): string {
  const agent = agentName(r.about.host.agent);
  const owner = assistingOwner(store.world.peek(), r.about.machine);
  return owner !== null ? said.someoneAssisting(agent, owner) : said.someone(agent, r.about.machine);
}

/** Where a conversation is said to be: its machine, or whose agent it is. */
function placeSaid(r: ResidentNow): string {
  return assistingOwner(store.world.peek(), r.about.machine) !== null ? someoneSaid(r) : r.about.machine;
}

function askForget(r: ResidentNow, then?: () => void): void {
  const words = talkMenuWords;
  const shown = talkActs.view(r);
  const name = titleOf(shown) ?? someoneSaid(r);
  const working = !isUnreached(r) && (r.now?.activity === "busy" || !!r.now?.turn);
  const also = [working ? words.deleteWorking : "", isUnreached(r) ? words.deleteAway(placeSaid(r)) : ""].filter(Boolean);
  forgetConfirm.ask({
    title: words.deleteTitle(name),
    what: words.deleteWhat,
    fields: `<ul class="forget-parts">${words.deleteParts(words.hostRecord(r.about.host.agent)).map((part) => `<li>${escapeText(part)}</li>`).join("")}</ul>${also.length ? `<p class="forget-also">${escapeText(also.join(" "))}</p>` : ""}`,
    go: words.deleteGo,
    act: async () => talkActs.delete(r),
    done: () => then?.(),
  });
}

/** A conversation page's "⋯": the conversation as the World lists it now,
 *  renamed in the page's own title; deleted, the page is left for the front
 *  page. */
function talkManage(address: Address, page: () => ConversationPage, markdown?: () => Promise<string>) {
  return {
    words: { more: talkMenuWords.more, archived: talkMenuWords.archivedNote, restore: talkMenuWords.restore, deleted: talkMenuWords.deletedNote },
    open: (anchor: HTMLElement) => {
      const r = listedAt(store.world.peek(), address);
      if (!r) return;
      const here = store.world.peek()?.here ?? (live ? null : specimen.world.here);
      // Begun here from Fastctx, it opens in its agent's own program too; a
      // specimen only shows where that would be.
      const own = r.about.world_started && r.about.machine === here && (desktopAvailable() || !live);
      talkMenu.open(r, {
        anchor,
        rename: () => page().startRename(),
        find: () => page().showFind(),
        steps: { shown: () => page().stepsShown(), toggle: () => page().toggleSteps() },
        deleted: () => replacePlace(link({ kind: "home" })),
        ...(markdown
          ? {
              copy: () =>
                void markdown()
                  .then((text) => navigator.clipboard.writeText(text))
                  .then(
                    () => page().notify(talkMenuWords.copied),
                    (error: unknown) => page().notify(String(error), true),
                  ),
            }
          : {}),
        ...(own
          ? {
              attach: {
                agent: agentName(r.about.host.agent),
                run: () => {
                  page().notify(talkMenuWords.openedOwn);
                  (live ? attachHere(address.resident) : Promise.resolve()).catch((error: unknown) => page().notify(String(error), true));
                },
              },
            }
          : {}),
      });
    },
    restore: () => {
      const r = listedAt(store.world.peek(), address);
      if (r) talkActs.restore(r);
    },
  };
}

/** Whether the conversation at `address` is archived or gone, as the World
 *  and what the user did to it say; `listed` is whether it was ever listed
 *  while its page was open, so a page opened before the World answered is
 *  not taken for one deleted. */
function presenceOf(world: WorldNow | null, address: Address, listed: { ever: boolean }): "removed" | "deleted" | null {
  const r = listedAt(world, address);
  if (!r) return listed.ever ? "deleted" : null;
  listed.ever = true;
  return talkActs.view(r).removed ? "removed" : null;
}

/** A specimen's conversations take what is done to them after a moment,
 *  as a machine would, and are listed so. */
function specimenActOn(address: Address, change: (r: ResidentNow) => ResidentNow | null): Promise<void> {
  return new Promise((resolve) =>
    setTimeout(() => {
      for (const machine of specimen.world.who.machines) {
        if (machine.kind !== "answered" || machine.reply.machine !== address.machine) continue;
        machine.reply.residents = machine.reply.residents.flatMap((r) => {
          if (r.about.id !== address.resident) return [r];
          const changed = change(r);
          return changed ? [changed] : [];
        });
      }
      resolve();
    }, 700),
  );
}

async function specimenManaged(address: Address, action: Management): Promise<Managed> {
  await specimenActOn(address, (r) => {
    if (action.do === "remove") return { ...r, removed: new Date().toISOString() };
    if (action.do === "restore") return { ...r, removed: null };
    if (action.do === "move") return { ...r, town: action.town };
    return r;
  });
  const now = residentAt(specimen.world, address);
  return { removed: now?.removed ?? null, town: now?.town ?? null, activity: now?.now?.activity ?? null, steps: [], notification: null };
}

async function specimenRenamed(address: Address, name: string): Promise<Controlled> {
  await specimenActOn(address, (r) => (r.now ? { ...r, now: { ...r.now, title: name } } : r));
  return { kind: "done", said: null };
}

/** A specimen's agent takes an answer a while after it is given. */
async function specimenAnswered(address: Address): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 1600));
  await specimenActOn(address, (r) => {
    if (!r.now) return r;
    const { asked: _answered, ...now } = r.now;
    return { ...r, now };
  });
  void store.refresh();
}

async function specimenForgot(address: Address): Promise<Forgetting> {
  await specimenActOn(address, () => null);
  return "asked";
}

/** Nothing here is under way, so a new version may be installed now: no
 *  agent of this machine in a turn, and nobody typing. */
function idleHere(): boolean {
  const world = store.world.peek();
  const here = machinesOf(world).find((machine) => machine.here)?.name;
  const busy = residentsOf(world).some((r) => r.now?.activity === "busy" && (!here || r.about.machine === here));
  const active = document.activeElement;
  const typing = (active instanceof HTMLTextAreaElement || active instanceof HTMLInputElement) && active.value.trim() !== "";
  return !busy && !typing;
}

const updateWords = english ? enUpdate : zhUpdate;
/** Where updates are asked: this device's node or device, or the specimen's
 *  stand-in, one for the page's whole life so what it keeps holds. */
const specimenUpdate = live ? null : specimenUpdates(params, params.has("phone"));
/** Each login's update view of the machines page, from its hub. */
const askUpdatesView = live ? networkUpdates : specimenUpdateView(() => store.world.peek());
const updatesAsked = specimenUpdate ? specimenUpdate.seam : updateSeam;
const appUpdates = (() => {
  const updates = new Updates(updatesAsked, idleHere);
  if (specimenUpdate) updates.know(specimenUpdate.version);
  else
    void getVersion().then(
      (version) => updates.know(version),
      (error: unknown) => console.error("reading this application's version", error),
    );
  return updates;
})();
/** Has a machine of a release before 1.0.1 beta 3 update itself to the
 *  newest on its line: what such a machine takes. */
const updateOlderMachine = (name: string) => updatesAsked.machine(name);
const updateCard = new UpdateCard(appUpdates, updateWords, live ? !onComputer() : params.has("phone"), {
  view: english ? enUpdateView : zhUpdateView,
  // The other machines this device's first account that the hub answers
  // for may update, that can take a newer version.
  count: async () => {
    const world = store.world.peek();
    // Every login's view, each machine counted once.
    const able = new Set<string>();
    for (const login of loginsOf(world?.accounts)) {
      const reply = await askUpdatesView(login.hubName, login.account, { do: "view" }).catch(() => null);
      if (reply?.kind !== "view") continue;
      for (const one of reply.view.machines) if (one.machine !== world?.here && one.offer && one.actions.schedule.allowed) able.add(one.machine);
    }
    return able.size;
  },
  open: () => {
    location.hash = link({ kind: "machines", updates: true });
  },
}, {
  versions: (look) => versionsHere(look),
  relays: live ? onComputer() : !params.has("phone"),
  // The World's program here: put in place by the interface's, then the
  // node asked to hand over to it.
  renewing: live
    ? onComputer()
      ? {
          state: renewalState,
          renew: async () => {
            await placeFastctx();
            await renewHere();
          },
        }
      : null
    : params.has("phone")
      ? null
      : specimenComputer().renewing,
  settings: english ? enSettings : zhSettings,
});
const versionWindow = new VersionWindow(updateWords.versions);
// A picture attached and not yet sent opens in the same viewer as the
// conversation's pictures.
showAttachedWith((images, index) => viewPictures(images, index, (english ? en : zh).pictures));

/** The window of versions picking the network's lowest version, from the
 *  releases this device knows. */
function minimumVersions(current: string | null, pick: (version: string | null) => void): void {
  const state = appUpdates.state.peek();
  if (!state) return;
  versionWindow.open(
    { who: null, running: appUpdates.version, state, look: () => appUpdates.look().then(() => appUpdates.state.peek() ?? state) },
    { kind: "minimum", current, pick: async (version) => pick(version) },
  );
}

/** The window of versions for this device: a version picked comes as the
 *  newest would, its card saying how. */
function versionsHere(look: boolean): void {
  const state = appUpdates.state.peek();
  if (!state) return;
  versionWindow.open(
    {
      who: null,
      running: appUpdates.version,
      state,
      look: () => appUpdates.look().then(() => appUpdates.state.peek() ?? state),
    },
    { kind: "update", pick: (version) => appUpdates.pick(version).then(() => updateCard.open()) },
    look,
  );
}

// A switch's chosen side slides to the one pressed, wherever it is drawn.
followSwitches();

const shell = new Shell({
  store,
  words: shellWords,
  sections: english ? enProfile : zhProfile,
  updates: { offered: appUpdates.offered, open: () => updateCard.open(), words: updateWords },
  newTalk: () => beginTalk(place.kind === "project" || place.kind === "new" ? place.town : null),
  newProject: (login) => projectForm.open(null, login),
  acts: talkActs,
  menu: talkMenu,
  menuWords: talkMenuWords,
  projects: {
    newTalk: (town) => beginTalk(town),
    edit: (town) => {
      const record = store.world.peek()?.towns.find((one) => one.town === town)?.record;
      if (record) projectForm.open({ town, record });
    },
    acts: (town) => projectActs.acts(town),
    run: (act, town, name) => projectActs.run(act, town, name),
  },
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
  return {
    here: world.here,
    machines: world.who.machines,
    known: [...residentsOf(world, true), ...unreachedOf(world, true)],
    others: world.who.others ?? [],
    nodes: machinesOf(world),
    assisting: new Map(assistantsOf(world).map((one) => [one.machine, one.owner ?? ""])),
    towns: world.towns,
    away,
  };
}

/** The parts of the World's present whose change leaves something behind
 *  to tell: a turn ending, a job going, a machine coming or going, someone
 *  arriving. */
function shapeOf(world: WorldNow): string {
  return JSON.stringify([
    residentsOf(world).map((r) => [r.about.id, r.about.machine, r.now?.activity ?? null, r.now?.turn ?? null, r.town]),
    jobsOf(world).map((job) => job.job),
    machinesOf(world).map((machine) => [machine.written, machine.online]),
    world.towns.map((town) => town.town),
  ]);
}

const focusWords = english ? enFocus : zhFocus;
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

/** A conversation as Markdown, named as its page names it: all of it, the
 *  parts the page does not hold read from the shell. */
function exported(conversation: Conversation, someone: Someone): () => Promise<string> {
  return () =>
    markdownOf(conversation, words, {
      title: conversation.title ?? (someone.assistFrom ? shellWords.someoneAssisting(someone.agent, someone.assistFrom) : `${someone.agent} ${someone.machine}`),
      agent: someone.agent,
      you: homeWords.you,
      plan: words.plan,
      waiting: words.waitingForYou,
    });
}

/** "Claude laptop" for a resident the World knows, archived or away; its
 *  id where it does not. */
function someoneAt(address: Address): string {
  const known = listedAt(store.world.peek(), address);
  return known ? someoneSaid(known) : `${address.resident} ${address.machine}`;
}

/** The card about the agent in a conversation, and how to fill it. */
/** The machine the agent at `address` lives on, where an account it works
 *  for may work on it (not so for an agent assisting the user), and the login
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

/** The machines the conversation at `address` may be pointed at: where it
 *  lives and those an account it works for may work on; for an agent
 *  assisting the user, the account's own. */
function focusPlacesFor(address: Address): FocusPlace[] {
  const world = store.world.peek();
  const known = residentAt(world, address);
  const assistant = known ? assistantOf(world, known) : null;
  const found = livesFor(address);
  const machines = assistant ? filePlaces(world, assistant.login) : found.lives ? workPlaces(world, found.lives, found.login) : [];
  return machines.map((machine) => focusPlace(world, machine));
}

/** The machines and folders a conversation is pointed at, where the user
 *  may point it: one they opened from here and that is still here. */
function focusOf(known: ResidentNow | null): Focus | null {
  if (!known || known.about.origin.kind !== "user" || !known.about.world_started) return null;
  return known.about.focus ?? noFocus();
}

/** The account's MCP servers as the conversation at `address` switches
 *  them, where it works for one account the user opened it from: each switch
 *  is sent as it is turned, and shown turned until the World says
 *  otherwise. */
function conversationServers(address: Address): FocusServers | undefined {
  const { login } = livesFor(address);
  if (!login || !focusOf(residentAt(store.world.peek(), address))) return undefined;
  let turned: string[] | null = null;
  return {
    ...serverLists.switches(() => login),
    off: () => turned ?? residentAt(store.world.peek(), address)?.about.mcp_off ?? [],
    changed: (off) => {
      turned = off;
      if (!live) return;
      void manageResident(address, { do: "mcp", off })
        .then(() => store.refresh())
        .catch(() => {
          turned = null;
        });
    },
  };
}

/** The specimen's MCP switches: the account's servers, turned here alone. */
function specimenServers(): FocusServers {
  let off: string[] = ["figma"];
  const login = loginsOf(specimen.world.accounts)[0] ?? null;
  return { ...serverLists.switches(() => login), off: () => off, changed: (next) => (off = next) };
}

/** Points the conversation at `address` at machines and folders; what was
 *  not done of it is said in the user's words. */
async function pointAt(address: Address, focus: Focus): Promise<string[]> {
  const managed = await manageResident(address, { do: "focus", focus });
  void store.refresh();
  return managed.steps
    .filter((step): step is typeof step & { outcome: "pending" | "unsupported" | "failed" } => step.outcome !== "done")
    .map((step) => detailsWords.unfinished(step.stage, step.outcome, step.detail));
}

/** A specimen's folders: a few made up, the same on every machine. */
function specimenFolders(machine: string, path: string | null): Promise<Folders> {
  const home = "C:/Users/ada";
  const inside: Record<string, string[]> = {
    "": [home, "D:/"],
    [home]: [".cache", "code", "Documents", "Downloads"],
    [`${home}/code`]: ["fastctx", "notes", "playground"],
    "D:/": ["data", "models"],
  };
  const names = inside[path ?? ""] ?? [];
  const folders = names.map((name) => (path === null ? { name, path: name } : { name, path: `${path.replace(/\/$/, "")}/${name}`, hidden: name.startsWith(".") }));
  const parent = path === null || path === "D:/" ? null : path.slice(0, path.lastIndexOf("/")) || null;
  return new Promise((resolve) => setTimeout(() => resolve({ machine, path, parent, home, folders }), 280));
}

/** The projects a conversation can be in: those of the accounts it works
 *  for, wherever its agent lives. */
function townsFor(r: ResidentNow): { town: string; name: string }[] {
  const world = store.world.peek();
  if (!world) return [];
  const usable = new Set(groupsOf(world).filter((group) => residentIn(world, group, r)).flatMap((group) => group.towns));
  return world.towns.filter((town) => usable.has(town.town)).map((town) => ({ town: town.town, name: town.record.name }));
}

/** The machines page's drawing of `world`, as a picture on another page:
 *  nothing being added, framed or moved, `marked` circled. */
function stillScene(world: WorldNow, marked: string | null = null): MachineMapScene {
  const map = mapOf(world, english ? enMachines : zhMachines);
  return { ...map, adds: [], waiting: new Set(), selected: marked, fresh: new Set(), opening: new Set(), framed: [] };
}

function home(): Page {
  const lastVisit = live ? seen.read() : specimen.lastVisit;
  // The drawing of the World; a machine on it opens on the machines page,
  // a hub on the account page, a box the machines page.
  const drawing = new MachineMap({
    words: english ? enMachines : zhMachines,
    storageKey: MAP_ORDER,
    still: true,
    event: (event) => {
      if (event.kind !== "select" || !event.key) return;
      const { key } = event;
      location.hash = key.startsWith("hub:")
        ? link({ kind: "account" })
        : key.startsWith("group:")
          ? link({ kind: "machines" })
          : key.startsWith("assist:")
            ? link({ kind: "machines", focus: key.slice("assist:".length) })
            : link({ kind: "machine", name: key.slice("m:".length) });
    },
  });
  const page = new HomePage({
    drawing: drawing.element,
    words: homeWords,
    lastVisit,
    link: talkLink,
    head: head(""),
    newTalk: () => beginTalk(),
    stopJob: live ? (job, machine) => stopJob(job, machine) : async () => {},
    updates: () => updateCard.open(),
    seenTold: toldSeen,
    // An answer from here goes in once, however long the way is closed, as
    // one from a notification does (`world-network.md`, 断开不是结束).
    answer: (address, asked, choice) => {
      if (!live) return specimenAnswered(address);
      const id = crypto.randomUUID();
      return untilAnswered(() => talk(address, answerWith(asked, choice), [], null, id), () => {}, address.machine);
    },
  });
  let away: Away | null = live ? null : specimen.away;
  // What happened while the reader was away is asked of every machine when
  // the page opens, and again whenever the present changes in a way that
  // leaves something to tell; one question at a time. Asked again, only
  // what came after the last reading began is read, and added to what was:
  // whole again where a machine did not answer, or a new one is there.
  let shape = "";
  let asking = false;
  let again = false;
  let heard: { events: Map<string, SinceEvent>; machines: string; from: Date } | null = null;
  const ask = async (): Promise<void> => {
    if (!lastVisit) return;
    if (asking) {
      again = true;
      return;
    }
    asking = true;
    try {
      const known = heard;
      const answer = await since(known?.from ?? lastVisit, null);
      const answered = answer.machines.flatMap((machine) => (machine.kind === "answered" ? [machine.reply] : []));
      const machines = answer.machines.map((machine) => (machine.kind === "answered" ? machine.reply.machine : machine.machine)).sort().join(" ");
      const events = new Map(known && known.machines === machines ? known.events : []);
      for (const event of answer.events) events.set(event.id, event);
      const whole = answered.length === answer.machines.length && (!known || known.machines === machines);
      const began = Math.min(...answered.map((reply) => Date.parse(reply.read_started_at)));
      heard = whole && Number.isFinite(began) ? { events, machines, from: new Date(Math.max(lastVisit.getTime(), began - 2000)) } : null;
      if (known && known.machines !== machines) again = true;
      const read = happeningsOf({ ...answer, events: [...events.values()].sort((a, b) => Date.parse(a.at) - Date.parse(b.at)) });
      away = { happenings: read.happenings, unasked: read.unasked, trouble: null };
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
      // A computer in no network is the only machine: there is nothing to
      // draw, and nothing on the drawing to open.
      const alone = inNoNetwork(world);
      page.showDrawing(!alone);
      if (!alone) drawing.update(stillScene(world));
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
      drawing.dispose();
      if (live) seen.now();
    },
  };
}

function specimenTalk(address: Address): KeepablePage {
  let queuedHere: PendingControl[] = [];
  let toldHere = 0;
  // The shared input kept in the page; `?queued` puts two in its queue.
  let shared: SpecimenComposer | null = null;
  // `?retrying=<ms>`: words sent at rest wait for a host that does not
  // start, and after the delay their node says so, as it does after a minute.
  const retrying = params.has("retrying")
    ? (id: string) =>
        setTimeout(() => {
          const reason =
            sample.agent === "claude"
              ? `Claude Code is not installed on ${sample.machine}: no claude command`
              : "Codex is not installed on this machine: neither the ChatGPT desktop app nor a codex command was found";
          conversation.now(false, null, [{ id, state: "handing", host_retry: { since: new Date(Date.now() - 60_000).toISOString(), reason } }]);
          page.update();
        }, Number(params.get("retrying") || 1500))
    : undefined;
  const sharedInput = () =>
    (shared ??= specimenComposer(address, () => conversation.turning(), params.has("queued") ? (english ? ["Then run the overflow test on lab", "And write the result into the journal"] : ["然后在 lab 上跑溢出测试", "再把结果写进手记"]) : [], retrying));
  const name = address.resident;
  const sample: Specimen & { later?: SessionEvent[]; rate?: number; timed?: boolean } = name.startsWith("stream")
    ? streamSpecimen(params)
    : (specimens[name === "codex-a51c" ? "asking" : name === "claude-5teps" ? "steps" : name === "trips-1" ? "trips" : name === "deeds-1" ? "deeds" : name === "claude-t0ur" ? "tour" : name === "claude-c0ntrols" ? "controls" : name === "claude-v01c" ? "voice" : name.startsWith("codex") ? "codex" : "claude"] ?? specimens.claude!)();
  const conversation = new Conversation();
  conversation.thoughtWords = sample.agent !== "codex";
  // A specimen keeps its whole history as a shell would, and its page holds
  // the latest part of it (`specimen/transcript.ts`).
  const kept = new LocalTranscript(sample.events, Number(params.get("partDelay") ?? 30));
  const seed = seeds.get(talkKey(address));
  seeds.delete(talkKey(address));
  if (seed) conversation.sending(seed.text, sample.here, seed.attached);
  // `?answer`: the specimen agent answers what is sent to it.
  if (params.has("answer")) {
    const sending = conversation.sending.bind(conversation);
    let turns = 0;
    conversation.sending = (text, here, attached) => {
      const key = sending(text, here, attached);
      turns += 1;
      playTimed(answering(text, `answer-${turns}`), (event) => {
        conversation.happened(kept.add(event), event);
        page.update();
        return true;
      });
      return key;
    };
  }
  const known = residentsOf(specimen.world).find((r) => r.about.id === address.resident && r.about.machine === address.machine);
  const page: ConversationPage = new ConversationPage({
    words,
    here: sample.here,
    someone: {
      agent: sample.agent === "claude" ? "Claude" : "Codex",
      machine: sample.machine,
      model: sample.model,
      project: sample.town,
      heldByWorld: true,
      // A specimen with none listed is pointed at its own machine.
      focus: focusOf(known ?? null) ?? { machines: [{ machine: sample.machine, folders: [] }] },
    },
    head,
    link: talkLink,
    conversation,
    side: specimenSide(sideWords, planWords, { ...planSamples.blog!, facts: (id) => planSamples.blog!.facts[id] ?? null }, english),
    focus: {
      words: focusWords,
      places: () =>
        machinesOf(specimen.world).map((machine) => focusPlace(specimen.world, machine)),
      folders: specimenFolders,
      set: () => new Promise((resolve) => setTimeout(() => resolve([]), 600)),
      servers: specimenServers(),
    },
    opening: { open: notLive, reveal: notLive },
    pictures: {
      thumbnail: async (_machine, path, max) => specimenPicture(path, max),
      attachmentThumbnail: async (attachment, max) => specimenPicture("path" in attachment ? attachment.name : "picture", max),
      original: async (picture) => (await fetch(specimenPicture(picture.path ?? picture.name, 2400))).blob(),
      save: async () => true,
    },
    controls: {
      words: composerWords,
      // A compaction asked for here waits for a turn's end, as one asked
      // mid-turn does, so its line above the input can be looked at.
      offers: async () => ({ ...(sample.agent === "claude" ? specimenClaudeOffers : specimenOffers), control_queue: true, controls_told: true, pending_controls: queuedHere }),
      control: async (what) => {
        if (what.do === "cancel_control") {
          queuedHere = queuedHere.filter((row) => row.id !== what.id);
          shared?.forget(what.id);
          return { kind: "done", said: null };
        }
        const name = what.do === "command" ? what.name : what.do;
        if (name === "compact" && conversation.turning()) {
          const id = `control-${Date.now()}-${queuedHere.length + 1}-0`;
          queuedHere = [...queuedHere, { id, control: what, at: new Date().toISOString(), state: "waiting" }];
          shared?.place(id);
          return { kind: "reported", said: "Waiting for the current turn to end.", queued: id };
        }
        // Told in the conversation as it begins and ends, as a node that
        // tells commands does: a compaction a moment later, `/goal` with an
        // answer to open, `/init` refused, `/fast` with a word of the World's.
        const row: PendingControl = { id: `control-${Date.now()}-${(toldHere += 1)}-0`, control: what, at: new Date().toISOString(), state: "running" };
        const tell = (next: PendingControl) =>
          feed?.({ at: new Date().toISOString(), event: "changed", turn: null, change: { kind: "metadata", metadata: { fastctx_control: next } as unknown as JsonValue } });
        tell(row);
        await new Promise((resolve) => setTimeout(resolve, name === "compact" ? 2500 : 1200));
        if (name === "init") {
          const error = "AGENTS.md could not be written: the specimen writes nothing";
          tell({ ...row, state: "failed", error });
          throw { message: error, code: "told" };
        }
        if (name === "compact") {
          const item = { id: `compact-${Date.now()}`, kind: "compaction" as const };
          feed?.({ at: new Date().toISOString(), event: "began", turn: "specimen-compact", item });
          feed?.({ at: new Date().toISOString(), event: "finished", turn: "specimen-compact", item });
        }
        const said =
          name === "goal"
            ? JSON.stringify({ goal: { objective: "Ship the overflow fix with its test", status: "active", tokens_used: 41230, started_at: "2026-10-21T09:12:00Z" } })
            : name === "fast"
              ? "Service tier priority saved for the next World turn."
              : null;
        const done: Controlled = { kind: "done", said };
        tell({ ...row, state: "completed", result: done });
        return done;
      },
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
    viewKey: `view:${address.resident}@${address.machine}`,
    composer: () => ComposerSession.open(address, { transport: sharedInput() }),
    files: { words: composerWords.files, find: specimenFiles },
    manage: talkManage(address, () => page, exported(conversation, { agent: sample.agent === "claude" ? "Claude" : "Codex", machine: sample.machine, model: sample.model, project: sample.town })),
    send(text, attached) {
      conversation.sending(
        text,
        sample.here,
        attached.shown.map(({ name, size, image, preview }) => ({ name, size, image, preview })),
      );
      page.update();
    },
    // A specimen's agent never stops: the button shows it stopping.
    stop: () => new Promise<boolean>(() => {}),
    stopJob: async () => {},
    jobOutput: async (job) => specimenOutput(job.command),
    lines: async (_machine, path) => specimenLines(path),
    agentRecord: (agent) => specimenAgentRecord(agent),
    calls: (turn) => void openCalls(known?.about ?? { machine: address.machine, host: { agent: sample.agent } }, address, turn),
  });
  // What it started that still runs, as the specimen World lists it.
  // With `?sentinels`, a sentinel the conversation left stands beside them.
  const specimenSentinels: PanelJob[] = params.has("sentinels")
    ? [
        {
          job: "s-a1b2c3",
          machine: "server",
          started_at: "",
          command: english ? "Wake me when the training loss falls below 0.3" : "训练的 loss 降到 0.3 以下时叫醒我",
          output: "",
          sentinel: {
            phase: words.sentinelPhase("watching"),
            plan: english ? "Reads the last lines of train.log every ten minutes" : "每十分钟读一次 train.log 的最后几行",
            seen: "step 4200  loss 0.412",
          },
        },
        {
          job: "s-d4e5f6",
          machine: "server",
          started_at: "",
          command: english ? "Wake me at eight tomorrow to read the night's regression" : "明早八点叫醒我看夜里的回归",
          output: "",
          sentinel: {
            phase: words.sentinelPhase("watching"),
            plan: english ? "Waits for the time; looks at nothing before it" : "只等时刻，之前什么都不看",
            seen: null,
            wakes: [8, 20].map((hour) => {
              const at = new Date(store.clock.peek());
              at.setDate(at.getDate() + 1);
              at.setHours(hour, 0, 0, 0);
              return at.getTime();
            }),
          },
        },
      ]
    : [];
  const runningJobs = effect(() => {
    const now = store.clock.get();
    page.running(
      [
        ...jobsOf(specimen.world).filter((job) => {
          const owner = ownerResident(job.owner);
          return owner?.resident === address.resident && owner.machine === address.machine;
        }),
        ...specimenSentinels,
      ],
      now,
    );
  });
  let left = false;
  /** Hands the page one more event of the specimen's, once it reads them. */
  let feed: ((event: SessionEvent) => boolean) | null = null;
  if (params.has("live") && !sample.later) {
    let index = 0;
    const next = () => {
      const event = sample.events[index++];
      if (!event || left) return;
      conversation.event(event);
      page.update();
      setTimeout(next, event.event === "heard" ? 900 : 420);
    };
    next();
  } else if (params.has("events")) {
    // `?events=<address>`: a conversation's events, one a line, read whole
    // as a shell would keep them, for looking at a real one.
    void fetch(params.get("events")!)
      .then((answer) => answer.text())
      .then((text) => {
        const events = text.split("\n").filter(Boolean).map((line) => JSON.parse(line) as SessionEvent);
        const read = new LocalTranscript(events, Number(params.get("partDelay") ?? 30));
        if (left) return;
        conversation.open(read.window(), read);
        page.update({ read: true });
      });
  } else {
    conversation.open(kept.window(), kept);
    page.update({ read: true });
    // What a made-to-measure specimen does next, each event its own task at
    // its rate, as a shell's channel hands them over.
    const take = (event: SessionEvent) => {
      if (left) return false;
      conversation.happened(kept.add(event), event);
      page.update();
      return true;
    };
    feed = take;
    if (sample.later?.length) {
      if (sample.timed) playTimed(sample.later, take);
      else playLater(sample.later, sample.rate ?? 0, take);
    }
  }
  // Archived or deleted from its "⋯", as a live page follows it.
  const listedEver = { ever: false };
  let suspended = false;
  const following = effect(() => {
    const now = store.world.get();
    talkActs.changed.get();
    if (!left) page.presence(presenceOf(now, address, listedEver));
    if (!left && !suspended) noteTold(now, address);
  });
  return {
    element: page.element,
    title: sample.title,
    suspend: () => {
      suspended = true;
      page.suspend();
    },
    reach: (turn: string) => page.showTurn(turn),
    resume: () => {
      suspended = false;
      noteTold(store.world.peek(), address);
      page.resume();
    },
    leave() {
      left = true;
      runningJobs();
      following();
      page.leave();
    },
  };
}

/** For a conversation an agent of another's works in for the user: where
 *  its files other than images go, one of the user's own machines, as the
 *  World is now (`world-run.md`, 受协助的人附的文件). */
function assistantFiles(address: Address, nowhere: string): AssistantFiles {
  const assistant = () => {
    const world = store.world.peek();
    const known = residentAt(world, address);
    return known ? assistantOf(world, known) : null;
  };
  return {
    assisting: () => !!assistant(),
    place: () => {
      const one = assistant();
      return one ? fileHome(store.world.peek(), one.login) : null;
    },
    nowhere,
  };
}

/** Where a file this device attaches to `address` goes, when an agent of
 *  another's works there for the user; none for any other conversation. */
function filePlace(address: Address): FilePlace | null {
  const world = store.world.peek();
  const known = residentAt(world, address);
  const assistant = known ? assistantOf(world, known) : null;
  const filesOn = assistant ? fileHome(world, assistant.login) : null;
  return assistant && filesOn ? { account: { hub: assistant.login.hub, account: assistant.login.account }, filesOn } : null;
}

/** Who a resident is, as the World last listed it, removed or not; null
 *  while it is not listed yet. */
function someoneOf(world: ReturnType<typeof store.world.peek>, address: Address): Someone | null {
  const known = residentAt(world, address);
  if (!known) return null;
  const assistant = assistantOf(world, known);
  return {
    agent: agentName(known.about.host.agent),
    machine: address.machine,
    assistFrom: assistant ? (assistant.owner ?? assistant.machine) : null,
    focus: known.removed ? null : focusOf(known),
    model: known.about.model ?? null,
    project: known.town ? (world?.towns.find((t) => t.town === known.town)?.record.name ?? known.town) : null,
    heldByWorld: known.about.world_started,
  };
}

/** A conversation's page; `inPanel` is a colleague's, opened in the panel
 *  beside the conversation that asked it, under the panel's own head. */
function liveTalk(address: Address, inPanel = false): KeepablePage {
  const world = store.world.peek();
  const listed = someoneOf(world, address);
  const someone: Someone = listed ?? { agent: address.resident, machine: address.machine, model: null, project: null };
  // Codex's own app, editor or command line holds a conversation the user
  // opened there: the World cannot drive a goal in it.
  const known = residentAt(world, address);
  const noGoal = !!known && /^codex/i.test(known.about.host.agent) && !known.about.world_started;
  const here = world?.here ?? null;
  const conversation = new Conversation();
  // The agent by its name, or, not yet listed, its resident's id.
  conversation.thoughtWords = !/^codex/i.test(someone.agent);
  // A conversation just begun here opens with its first words on their way.
  const seed = seeds.get(talkKey(address));
  seeds.delete(talkKey(address));
  // The World answered for them as it opened the conversation.
  if (seed) conversation.named(conversation.sending(seed.text, here, seed.attached), null);
  // Names of files for an `@`, on the agent's machine, in its places.
  const search = new FileSearch(address.machine, address.resident);
  const page: ConversationPage = new ConversationPage({
    words,
    here: here ?? "",
    someone,
    head: inPanel ? quietHead : head,
    link: talkLink,
    conversation,
    side: liveSide({
      words: sideWords,
      planWords,
      store,
      address,
      bare: inPanel,
      noGoal,
      took: (ms) => words.took(ms),
      openId: (id) => openWorldId(id),
      open: (colleague) => {
        const page = liveTalk(colleague, true);
        page.resume();
        return { element: page.element, leave: () => page.leave() };
      },
    }),
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
    viewKey: `view:${address.resident}@${address.machine}`,
    composer: () => ComposerSession.open(address),
    focus: {
      words: focusWords,
      places: () => focusPlacesFor(address),
      folders: (machine, path) => foldersOf(machine, path),
      set: (focus) => pointAt(address, focus),
      servers: conversationServers(address),
    },
    // A file elsewhere is brought to this device first; one here opens
    // where it is.
    opening: {
      open: async (machine, path, held) => openLocal(await fetchFile(machine, path, held)),
      openAttachment: async (attachment, held) => openLocal((await fetchAttachment(attachment, held)).path),
      reveal: onComputer() ? reveal : undefined,
    },
    pictures: {
      thumbnail: async (machine, path, max) => thumbnailUrl(await thumbnail(machine, path, max)),
      attachmentThumbnail: async (attachment, max) => thumbnailUrl(await attachmentThumbnail(attachment, max)),
      // A file the user sent is asked for as large as it is known to be,
      // which a gone original's host record answers too.
      original: async (picture, max, held) => {
        if (picture.attachment) return blobOf(await attachmentThumbnail(picture.attachment, max));
        const read = await readBytes(picture.machine, picture.path!, held);
        return new Blob([Uint8Array.from(atob(read.base64), (char) => char.charCodeAt(0))]);
      },
      save: async (picture, held) =>
        savedOf(await saveAs(picture.attachment ? (await fetchAttachment(picture.attachment, held)).path : await fetchFile(picture.machine, picture.path!, held), picture.name)),
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
      filesOn: assistantFiles(address, composerWords.attach.imagesOnly),
      seam: {
        upload: async (source, _name, progress) => {
          const target: UploadTarget = { kind: "resident", resident: address.resident };
          const place = filePlace(address);
          if (source.kind === "path") return uploadPath(address.machine, target, source.path, progress, place);
          const done = await uploadBytes(address.machine, target, source.file, place);
          progress(source.file.size);
          return done;
        },
      },
      drops: windowDrops,
    },
    manage: talkManage(address, () => page, exported(conversation, someone)),
    send(text, attached) {
      const key = conversation.sending(
        text,
        here,
        attached.shown.map(({ name, size, image, preview }) => ({ name, size, image, preview })),
      );
      page.update();
      // Words with no answer are said again under the same id until one
      // comes: they go in once (`world-network.md`, 断开不是结束).
      const id = crypto.randomUUID();
      return untilAnswered(
        () => talk(address, text, attached.refs as Attachment[], attached.filesOn ?? null, id),
        (why) => {
          conversation.sent(key, { state: "waiting", why });
          page.update();
        },
        address.machine,
      )
        .then(({ to, landed, delivery }) => {
          conversation.named(key, delivery ?? null);
          // Words in a turn take their place when the session hears them;
          // words in no turn yet say where they are.
          if (standsUnder(landed, someone.heldByWorld === true)) {
            const why = "why" in landed ? landed.why : null;
            conversation.sent(key, { state: "landed", how: landed.kind, machine: to.machine, why });
            page.update();
          }
          // A turn whose end this conversation will not see leaves nothing
          // for words queued after these to wait on.
          return landed.kind === "uncertain" || (landed.kind === "waiting" && !landed.answered) ? ("unseen" as const) : ("turn" as const);
        })
        .catch((error: unknown) => {
          conversation.sent(key, { state: "failed", why: whyOf(error) });
          page.update();
          return "failed" as const;
        });
    },
    stop: () => stop(address),
    // A sentinel's row withdraws it, then the list is asked again.
    stopJob: (job, machine) => (sentinelRows.peek().some((row) => row.job === job) ? sentinelStop(job, machine).then(() => readSentinels()) : stopJob(job, machine)),
    jobOutput: (job) => RemoteLines.open(job.machine, job.output, { kind: "output", job: job.job }),
    lines: (machine, path, decoding) => RemoteLines.open(machine, path, decoding),
    agentRecord: (agent) => agentActivity(address, agent),
    calls: (turn) => {
      const about = residentAt(store.world.peek(), address)?.about;
      if (about) void openCalls(about, address, turn);
    },
  });
  let left = false;
  // The sentinels it left, as rows beside its jobs; the goal's own sentinel
  // goes with the goal and is not withdrawn by hand.
  const sentinelRows = signal<PanelJob[]>([]);
  const sentinelRow = (one: SentinelSummary): PanelJob => ({
    job: one.definition.id,
    machine: one.definition.home,
    started_at: "",
    command: one.definition.condition || one.plan,
    output: "",
    sentinel: { phase: words.sentinelPhase(one.phase), plan: one.plan, seen: one.latest?.text ?? one.reason, wakes: one.definition.at ?? [] },
  });
  const readSentinels = (): Promise<void> =>
    sentinelList(address).then(
      (listed) => {
        if (!left) sentinelRows.set(listed.filter((one) => one.definition.kind !== "goal").map(sentinelRow));
      },
      // A home that cannot be asked now keeps the rows last seen.
      () => {},
    );
  // What it started that still runs, as the World last answered, and the
  // sentinels it left.
  const running = effect(() => {
    const world = store.world.get();
    const now = store.clock.get();
    const jobs = jobsOf(world).filter((job) => {
      const owner = ownerResident(job.owner);
      return owner?.resident === address.resident && owner.machine === address.machine;
    });
    page.running([...jobs, ...sentinelRows.get()], now);
  });
  // The sentinels' homes are asked, not followed: as the page opens, every
  // half minute while it is open, and after one is withdrawn.
  const sentinelTimer = window.setInterval(() => void readSentinels(), 30_000);
  void readSentinels();
  // The conversation as the World's watch reads it (`conversation/source.ts`):
  // the page holds its latest part and reads the rest as the reader comes to
  // it; what is new is shown at once. When the host replaced or removed
  // turns, or what came does not follow what is held, all of it is read
  // again.
  // Whether its machine tells what holds of it now (`Fold.now`): one too old
  // to is followed by the World's listing of it instead, once the page has
  // heard nothing for a while.
  let nowTold = false;
  let heardAt = Date.now();
  const reading = watched.open(address, {
    window: (window, parts) => page.reopened(() => conversation.open(window, parts)),
    extended: (start, events) => {
      if (!conversation.extended(start, events)) reading.again();
      page.update({ read: true });
    },
    rebased: (generation, offset, turnsAdded) => {
      conversation.rebased(generation, offset, turnsAdded);
      page.update({ read: true });
    },
    happened: (at, event) => {
      heardAt = Date.now();
      if (!conversation.happened(at, event) || conversation.stale) reading.again();
      page.update();
    },
    interrupted: (why, since) => {
      // What the host's own agents did while it was broken is not told
      // again: each is read from its own record from here.
      conversation.broke();
      page.broken(why, since);
    },
    resumed: () => page.resumed(),
    ended: (why) => {
      page.lost(why);
      page.update();
    },
    refused: (why) => {
      page.refused(why);
      page.update();
    },
    now: (busy, turn, kept) => {
      nowTold = true;
      conversation.now(busy, turn, kept);
      page.update();
    },
    deleted: () => gone(),
  });
  // Deleted, as its machine answers or the World lists it: nothing of it is
  // watched any more, and its page is kept no longer than it is in view.
  let deleted = false;
  const gone = () => {
    if (deleted) return;
    deleted = true;
    reading.close();
    page.presence("deleted");
    keptTalks.gone(talkKey(address));
  };
  // Opened before the World first answered, as from a notification at
  // start: built again once it has, to name the agent and its project.
  // Otherwise the World's listing of it is followed in place, without
  // reading the conversation again: named once listed, as one just started;
  // its project and model as they change; asleep or not.
  let shown = listed;
  const listedEver = { ever: false };
  let suspended = false;
  const named = !world
    ? effect(() => {
        if (!store.world.get()) return;
        queueMicrotask(() => {
          if (left) return;
          keptTalks.drop(talkKey(address));
          if (place.kind === "talk" && place.address.resident === address.resident && place.address.machine === address.machine) show(place);
        });
      })
    : effect(() => {
        const now = store.world.get();
        talkActs.changed.get();
        const presence = deleted ? "deleted" : presenceOf(now, address, listedEver);
        if (!left) page.presence(presence);
        if (!left && presence === "deleted") queueMicrotask(gone);
        const known = residentAt(now, address);
        const who = someoneOf(now, address);
        if (!left && !suspended) noteTold(now, address);
        if (
          !left &&
          !nowTold &&
          known?.now &&
          known.now.activity !== "busy" &&
          !known.now.turn &&
          conversation.busy() &&
          Date.now() - heardAt > QUIET_BEFORE_LISTED
        ) {
          conversation.now(false, null, []);
          page.update();
        }
        if (!known || !who || left) return;
        if (shown && who.agent === shown.agent && who.project === shown.project && who.model === shown.model && JSON.stringify(who.focus) === JSON.stringify(shown.focus)) return;
        shown = who;
        page.identify(who);
      });
  return {
    element: page.element,
    title: `${someone.agent} ${someone.machine}`,
    suspend: () => {
      suspended = true;
      page.suspend();
    },
    reach: (turn: string) => page.showTurn(turn),
    resume: () => {
      suspended = false;
      noteTold(store.world.peek(), address);
      page.resume();
    },
    leave() {
      left = true;
      named();
      running();
      clearInterval(sentinelTimer);
      page.leave();
      search.close();
      reading.close();
    },
  };
}

/** What a conversation told the user counts as seen while its page is in
 *  view on this device (`world-voice.md`, 要紧的事). */
function noteTold(world: ReturnType<typeof store.world.peek>, address: Address): void {
  const told = residentAt(world, address)?.now?.told;
  if (told && document.visibilityState === "visible") sawTold(talkKey(address), told.id);
}

/** Opens what one of the world's ids names: a conversation, or a job's
 *  output on its machine. */
function openWorldId(id: string): void {
  const world = store.world.peek();
  const resident = residentsOf(world).find((r) => r.about.id === id);
  if (resident) {
    location.hash = talkLink(addressOf(resident));
    return;
  }
  const job = jobsOf(world).find((one) => one.job === id);
  if (job) location.hash = fileLink(job.machine, job.output, true);
}

/** The head of a colleague's page opened in the panel: the panel's own
 *  head names it, so the page's is held but not shown. */
function quietHead(title: string, actions = ""): string {
  return `<div class="cp-quiet-head" hidden>${title}<div>${actions}</div></div>`;
}

/** How long a page whose machine does not tell what holds of it now hears
 *  nothing before it holds to the World's listing instead. */
const QUIET_BEFORE_LISTED = 10_000;

/** The conversations looked at last, alive (`conversation/kept.ts`). */
const keptTalks = new KeptPages();

/** The menu of the requests' filters, made when first opened. */
let callsMenu: ActsMenu | null = null;

/** A turn to bring into view as its conversation is shown. */
let turnWanted: { key: string; turn: string } | null = null;

/** Opens a conversation at one of its turns: a request's, from its
 *  login's usage. */
function openTurn(address: Address, turn: string | null): void {
  turnWanted = turn ? { key: talkKey(address), turn } : null;
  location.hash = talkLink(address);
}

/** One turn's requests, on the usage page of the login its conversation
 *  works in: the default one's folder as that machine's logins were last
 *  read, or read now. */
async function openCalls(about: { machine: string; host: { agent: string }; home?: string | undefined }, address: Address, turn: string): Promise<void> {
  const agent = about.host.agent;
  if (agent !== "claude" && agent !== "codex") return;
  const machine = about.machine;
  const defaultOf = (logins: MachineLogins | null) => logins?.[agent].listed.find((login) => login.default)?.dir ?? null;
  let dir = about.home ?? defaultOf(keptRecord(machine));
  if (!dir) {
    const read = await (live ? agentLogins(machine) : specimenLogins(machine, specimen.now)).catch(() => null);
    const heard = read?.machines.find((one) => one.kind === "answered" && one.logins.machine === machine);
    dir = heard?.kind === "answered" ? defaultOf(heard.logins) : null;
  }
  if (dir) location.hash = link({ kind: "usage", machine, agent, dir, turn: { conversation: address.resident, turn } });
}
const talkKey = (address: Address) => `${address.resident}@${address.machine}`;

function projectPage(town: string): Page {
  const page = new ProjectPage({
    words: projectWords,
    home: homeWords,
    someone: (r) => someoneSaid(r, homeWords),
    town,
    head,
    newTalk: shellWords.newTalk,
    edit: shellWords.edit,
    begin: (town) => beginTalk(town),
    revise: (town, record) => projectForm.open({ town, record }),
    link: talkLink,
    pageWords,
    pageLink: (town, page) => link({ kind: "page", town, page, version: null }),
    memory: {
      words: memoryWords,
      link: (town, name) => link({ kind: "memory", town, name }),
      write: (town) => journalForm.open(town),
      writer: (author) => memoryWriter(author).name,
    },
    machineLink: (name) => link({ kind: "machine", name }),
    phase: (phase) => words.sentinelPhase(phase),
    seenTold: toldSeen,
    more: {
      label: projectActsWords.more,
      shown: (town) => projectActs.acts(town).length > 0,
      open: (town, name, anchor) => projectActs.open(town, name, anchor),
      sharedBy: (town) => {
        const owner = projectActs.sharedBy(town);
        return owner ? projectActsWords.sharedBy(owner) : null;
      },
    },
  });
  // Its memory, where the feature is on: the catalog as it stands, then as
  // anyone writes to it. The switch is asked as the page opens, not taken
  // from a read that may not have come back yet.
  let memoryLeft = false;
  let memoryWatch: number | null = null;
  const followMemory = async () => {
    if (!live) return page.showMemory(memoryOn ? specimenCatalog(specimen.now) : null);
    memoryOn = await memorySeam.enabled(null, null).catch(() => memoryOn);
    if (memoryLeft || !memoryOn) return page.showMemory(null);
    try {
      const id = await memorySeam.watch(town, (catalog) => {
        if (!memoryLeft) page.showMemory(catalog);
      });
      if (memoryLeft) void unwatch(id);
      else memoryWatch = id;
    } catch {
      // A World without the watch still answers a single read.
      await memorySeam.catalog(town).then(
        (catalog) => !memoryLeft && page.showMemory(catalog),
        () => !memoryLeft && page.showMemory(null),
      );
    }
  };
  void followMemory();
  if (!live) {
    const map = specimenMap();
    page.showPlans(specimenHubPlans());
    page.showSenses(specimenHubSenses());
    page.update(map, { machine: specimen.world.here, scope: { kind: "town", town, record: null }, machines: specimen.world.who.machines.map((machine) => machine.kind === "answered" ? { ...machine, reply: { ...machine.reply, residents: machine.reply.residents.filter((r) => r.town === town) } } : machine) }, specimen.now);
    return { element: page.element, title: map.record.name };
  }
  let timer = 0;
  let left = false;
  // Each conversation's plan, read again when the conversation first shows
  // or its state moves (its plan is written as it works), and the senses
  // its conversations keep, listed again every half minute.
  const plans = new Map<string, string | null>();
  const planMoments = new Map<string, string>();
  const readPlans = async (who: WorldWho) => {
    const own = who.machines
      .flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : []))
      .filter((r) => !r.removed && r.about.origin.kind === "user");
    const stale = own.filter((r) => planMoments.get(talkKey({ resident: r.about.id, machine: r.about.machine })) !== `${r.now?.activity}@${r.now?.since}`);
    if (!stale.length) return;
    await Promise.all(
      stale.map(async (r) => {
        const key = talkKey({ resident: r.about.id, machine: r.about.machine });
        planMoments.set(key, `${r.now?.activity}@${r.now?.since}`);
        const read = await planRead(r.about.id).catch(() => null);
        plans.set(key, read?.body?.markdown ?? null);
      }),
    );
    if (!left) page.showPlans(new Map(plans));
  };
  let sensesTimer = 0;
  const readSenses = async () => {
    if (left) return;
    if (document.visibilityState === "visible") {
      const listed = await sentinelListAll().catch(() => null);
      if (listed && !left) page.showSenses(listed);
    }
    if (!left) sensesTimer = window.setTimeout(readSenses, 30_000);
  };
  void readSenses();
  // Asked again every two seconds while in view, as the World is.
  const refresh = async () => {
    if (left) return;
    if (document.visibilityState === "visible") {
      try {
        const { map, who } = await readProject(town);
        if (!left) {
          shell.retitle(map.record.name);
          page.update(map, who, new Date());
          void readPlans(who);
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
      memoryLeft = true;
      if (memoryWatch !== null) void unwatch(memoryWatch);
      clearTimeout(timer);
      clearTimeout(sensesTimer);
    },
  };
}

function memoryPage(town: string, name: string): Page {
  const project = store.world.peek()?.towns.find((t) => t.town === town)?.record.name ?? town;
  const page = new MemoryPage({
    words: memoryWords,
    head,
    project: { href: link({ kind: "project", town }), name: project },
    name,
    read: (version) => (live ? memorySeam.read(town, name, version) : Promise.resolve(specimenDocument(name, version, specimen.now))),
    save: (text, expected) => (live ? memorySeam.save(town, "spec", name, text, expected) : Promise.reject(new Error("specimen"))),
    writer: (author) => memoryWriter(author),
    now: () => (live ? new Date() : specimen.now),
  });
  return { element: page.element, title: name };
}

/** Who wrote a version of a document, and where their conversation is. */
function memoryWriter(author: MemoryAuthor): { name: string; href: string | null } {
  const by = author.by;
  if (by.kind === "user") return { name: memoryWords.you, href: null };
  if (by.kind === "resident") {
    const known = residentsOf(live ? store.world.peek() : specimen.world).find((r) => sameAddress(addressOf(r), by.address));
    // A conversation gone from the World is still named by its host.
    const agent = agentName(known?.about.host.agent ?? author.host?.agent ?? "");
    return { name: known ? someoneSaid(known, homeWords) : homeWords.someone(agent, by.address.machine), href: talkLink(by.address) };
  }
  if (by.kind === "page") return { name: by.page.page, href: null };
  return { name: agentName(by.agent), href: null };
}

/** The page open now, to hear the theme change. */
let openPage: PageScreen | null = null;
const pageConfirm = new Confirm({ close: pageWords.close, working: pageWords.working, failed: pageWords.failed });

/** The one of this device's logins a project's page is seen and agreed to
 *  as: its owner where logged in here, else the first it is shared with. */
function pageAccount(town: string): string | null {
  for (const hub of store.world.peek()?.accounts.hubs ?? []) {
    const project = hub.projects.find((one) => one.name === town);
    if (!project) continue;
    const able = hub.logins.filter((login) => login === project.owner || project.granted.includes(login));
    return able.find((login) => login === project.owner) ?? able[0] ?? null;
  }
  return null;
}

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
          read: (version) => readPage(town, name, version, pageAccount(town)),
          history: () => pageHistory(town, name),
          consent: (version, base, acts) => pageConsent(town, name, version, base, acts, pageAccount(town)),
          withdraw: (base) => withdrawPage(town, name, base),
          restore: (base, version) => restorePage(town, name, base, version),
        }
      : specimenPageWorld(name),
    tools: (view, reads) => {
      const tools = toolsFor({
        town,
        store,
        open: (address) => (location.hash = talkLink(address)),
        act: (action) => pageAct(town, name, view.publication.version, action, pageAccount(town)),
        reads,
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
  // The front page belongs to the workbench: without it, the interface opens on
  // the agents (`world-gui.md`, 界面的结构).
  if (next.kind === "home" && !deviceFeatures.conversations.peek()) next = { kind: "agents", machine: null };
  // A computer in no network has no machine details: the machines page
  // tells what a network is (`world-gui.md`, 机器的详情).
  if (next.kind === "machine" && inNoNetwork(store.world.peek())) {
    replacePlace(link({ kind: "machines" }));
    return;
  }
  const to = link(next);
  const replaced = replacedTo === to;
  replacedTo = null;
  if (!inner(next)) trail = [];
  else if (trail.length && link(trail[trail.length - 1]!.place) === to) trail.pop();
  else if (!replaced && shownPage && link(place) !== to) trail.push({ place, title: shownPage.title || placeName(place) });
  const came = inner(next) ? trail[trail.length - 1] : undefined;
  const parent = parentOf(next);
  // A phone logged in nowhere has only its way in, with nothing behind it.
  const onlyWayIn = next.kind === "login" && asDevice && !loginsOf(store.world.peek()?.accounts).length;
  backName = !inner(next) || onlyWayIn ? null : came ? came.title : placeName(parent);
  place = next;
  // A managed machine has one page, with nothing beside it (`world-gui.md`,
  // 受管机器上的界面).
  const managed = !!managedOf(store.world.peek()?.accounts);
  shell.alone(managed);
  let page: Page;
  if (managed) page = managedPage();
  else switch (next.kind) {
    case "home":
      page = home();
      break;
    case "talk": {
      const address = next.address;
      const key = talkKey(address);
      page = keptTalks.page(key, () => (live ? liveTalk(address) : specimenTalk(address)));
      if (turnWanted?.key === key) keptTalks.reach(key, turnWanted.turn);
      turnWanted = null;
      break;
    }
    case "new": {
      const talk = newTalkOf(next.town);
      const chosen = newTalkFor;
      newTalkFor = null;
      talk.show(chosen?.pick ?? null, chosen?.machine ?? null);
      page = { element: talk.element, title: talk.title, leave: () => talk.leave() };
      break;
    }
    case "project":
      page = projectPage(next.town);
      break;
    case "memory":
      page = memoryPage(next.town, next.name);
      break;
    case "page":
      page = pageScreen(next.town, next.page, next.version);
      break;
    case "machine": {
      const name = next.name;
      page = new MachinePage({
        store,
        words: english ? enMachinePage : zhMachinePage,
        machines: english ? enMachines : zhMachines,
        access: english ? enAccess : zhAccess,
        nameSaid: english ? enNameSaid : zhNameSaid,
        head,
        name,
        when,
        ago: (ms) => homeWords.ago(ms),
        stats: (machine, after, seen) => (live ? machineStats(machine, after, seen) : specimenStats(machine, after)),
        nameProblem: (text) => (live ? nameProblem("machine", text) : specimenNameProblem("machine", text)),
        change,
        log: live ? machineLog : specimenMachineLog,
        renameHere: async (to) => {
          if (live) return renameHere(to);
          const renamed = specimenRenameHere(store.world.peek(), to);
          if (renamed) store.world.set(renamed);
        },
        talk: (machine) => beginTalk(null, null, machine),
        confirm: accountConfirm,
        replace: (to) => replacePlace(link(to)),
        updates: {
          state: appUpdates.state,
          machine: updateOlderMachine,
          remote: (name, call) => updatesAsked.remote(name, call),
          here: () => updateCard.open(),
          words: updateWords,
          view: english ? enUpdateView : zhUpdateView,
        },
        manage: english ? enManage : zhManage,
        ...(live ? { allowPrivacy: () => void allowPrivacy().catch((error: unknown) => console.error("opening the Mac's privacy settings", error)) } : {}),
      });
      break;
    }
    case "machines":
      page = new MachinesPage({
        store,
        updates: {
          words: updateWords,
          view: english ? enUpdateView : zhUpdateView,
          ask: askUpdatesView,
          minimum: minimumVersions,
          state: appUpdates.state,
          remote: (name, call) => updatesAsked.remote(name, call),
          older: updateOlderMachine,
          versions: versionWindow,
        },
        batch: english ? enBatch : zhBatch,
        manage: english ? enManage : zhManage,
        words: english ? enMachines : zhMachines,
        access: english ? enAccess : zhAccess,
        people: english ? enPeople : zhPeople,
        head,
        focus: next.focus ?? null,
        ticked: !!next.updates,
        when,
        change,
        invite: (login) => tokens.open(login),
        talk: (machine) => beginTalk(null, null, machine),
        confirm: accountConfirm,
        log: live ? machineLog : specimenMachineLog,
        renaming: {
          words: english ? enMachinePage : zhMachinePage,
          nameSaid: english ? enNameSaid : zhNameSaid,
          check: (text) => (live ? nameProblem("machine", text) : specimenNameProblem("machine", text)),
          here: async (to) => {
            if (live) return renameHere(to);
            const renamed = specimenRenameHere(store.world.peek(), to);
            if (renamed) store.world.set(renamed);
          },
        },
        moving: {
          move: (login, request) => (live ? hubMove(login.hubName, login.account, request) : specimenMove(request)),
          arrive: () => (live ? arriveHub() : notLive()),
          // Over SSH only from a computer, as a server is added.
          remote:
            live && !onComputer()
              ? undefined
              : {
                  seams: live ? remoteSeams : specimenRemote(),
                  words: english ? enLogin : zhLogin,
                  token: (login, machine) =>
                    live
                      ? accountChange(login.hubName, login.account, { change: "token", register: true, name: machine, kind: "computer", expires: new Date(Date.now() + 3_600_000).toISOString(), way: "ssh" }).then(
                          (token) => token ?? Promise.reject(new Error("the hub made no token")),
                        )
                      : Promise.resolve("FX5SPECIMEN"),
                },
        },
      });
      break;
    case "agents": {
      const section = sectionOf(next) ?? "claude";
      keepSection(section);
      const profile = english ? enProfile : zhProfile;
      if (section === "prompt") {
        page = new PromptPage({ store, words: profile, head, calls: profileCalls });
        break;
      }
      if (!isHost(section)) {
        page = new McpPage({ store, words: profile, head, calls: profileCalls, confirm: accountConfirm, close: pageWords.close });
        break;
      }
      page = new AgentLoginsPage({
        store,
        words: english ? enAgentLogins : zhAgentLogins,
        changeWords: english ? enAgentChange : zhAgentChange,
        access: english ? enAccess : zhAccess,
        usage: english ? enUsage : zhUsage,
        head,
        agent: section,
        machine: next.machine,
        tab: tabOf(next),
        prefs: {
          words: profile,
          make: (agent) =>
            new HostPrefs({
              agent,
              words: profile,
              english,
              calls: profileCalls,
              models: agentModels,
              subagents:
                agent === "claude"
                  ? {
                      names: async (login) => {
                        const view = await subagentCalls(login.hubName, login.account, { operation: "list" });
                        return view.supported ? view.configuration.agents.map((agent) => agent.name) : null;
                      },
                      open: () => {
                        location.hash = link({ kind: "subagents" });
                      },
                    }
                  : undefined,
            }),
        },
        tabbed: (tab, agent) => {
          place = tab === "logins" ? { kind: "agents", machine: null, section: agent } : { kind: "agents", machine: null, section: agent, tab };
          history.replaceState(history.state, "", link(place));
          shell.mark(place);
        },
        showing: (machine, agent) => {
          // The address and the sidebar follow the machine and the agent
          // shown, without coming anew.
          place = { kind: "agents", section: agent, machine };
          keepSection(agent);
          history.replaceState(history.state, "", link(place));
          shell.mark(place);
        },
        when,
        workbench: deviceFeatures.conversations,
        read: (machine) => (live ? agentLogins(machine) : specimenLogins(machine, specimen.now)),
        scan: (machine, stop) => (live ? scanAgents(machine, stop) : specimenScan(machine, stop, specimen.now)),
        add: (machine, agent, dir) => (live ? addAgentLogin(machine, agent, dir) : specimenTakeUp(machine, agent, dir, specimen.now)),
        folder: (machine, dir, change) => (live ? agentFolder(machine, dir, change) : specimenFolder(machine, dir, change, specimen.now)),
        // A phone holds no folder of the World's; a plain browser walks the
        // specimen's folders.
        pick: live && onComputer() ? pickFolder : null,
        folders: (machine, path) => (live ? foldersOf(machine, path) : specimenFolders(machine, path)),
        preview: (machine, agent, dir, into, takeBack) => (live ? previewAgentChange(machine, agent, dir, into, takeBack) : specimenPreview(machine, agent, dir, into)),
        change: (machine, agent, dir, into, raise, takeBack) => (live ? changeAgentLogin(machine, agent, dir, into, raise, takeBack) : specimenLoginChange(machine, agent, dir, into)),
        limits: (machine, agent, dir) => (live ? readAgentLimits(machine, agent, dir) : notLive()),
        setTitle: (machine, agent, dir, on) => (live ? setAgentTitle(machine, agent, dir, on) : specimenSetTitle(machine, agent, dir, on, specimen.now)),
        usageOf: (machine, agent, dir) => (live ? agentUsage(machine, agent, dir) : specimenUsage(machine, agent, dir, specimen.now)),
        change_: change,
        confirm: accountConfirm,
      });
      break;
    }
    case "subagents":
      keepSection("claude");
      page = new SubagentsPage({
        store,
        head,
        words: english ? enProfile : zhProfile,
        english,
        call: subagentCalls,
        models: () => agentModels("claude"),
        confirm: accountConfirm,
        close: pageWords.close,
      });
      break;
    case "usage": {
      const usagePlace = next;
      page = new LoginUsagePage({
        store,
        words: english ? enUsage : zhUsage,
        logins: english ? enAgentLogins : zhAgentLogins,
        head,
        machine: next.machine,
        agent: next.agent,
        dir: next.dir,
        workbench: deviceFeatures.conversations,
        read: (machine) => (live ? agentLogins(machine) : specimenLogins(machine, specimen.now)),
        usage: (machine, agent, dir) => (live ? agentUsage(machine, agent, dir) : specimenUsage(machine, agent, dir, specimen.now)),
        when,
        talk: (machine, agent, home) => beginTalk(null, loginKey(machine, agent, home), machine),
        calls: {
          words: english ? enCalls : zhCalls,
          menu: (callsMenu ??= new ActsMenu()),
          ask: (machine, agent, dir, query) => (live ? agentCalls(machine, agent, dir, query) : specimenCalls(machine, agent, dir, query, specimen.now)),
          open: (machine, conversation, turn) => openTurn({ resident: conversation, machine }, turn),
          turn: next.turn ?? null,
          // The address no longer names the turn, and the page stays.
          unturn: () => {
            const { turn: _turn, ...all } = usagePlace;
            place = all;
            history.replaceState(history.state, "", link(all));
          },
        },
      });
      break;
    }
    case "talks":
    case "archived":
      page = new TalksPage({
        store,
        words: english ? enTalks : zhTalks,
        shell: shellWords,
        head,
        town: next.kind === "talks" ? next.town : null,
        archived: next.kind === "archived",
        place: () => place,
        acts: talkActs,
        menu: talkMenu,
        menuWords: talkMenuWords,
      });
      break;
    case "features":
    case "feature":
      page = new FeaturesPage({ store, words: english ? enFeatures : zhFeatures, homeWords: agentHomeWords, english, head, feature: next.kind === "feature" ? next.feature : null, agents: () => agentsOn(null), confirm: accountConfirm, memory: memorySwitch, sentinels: { seam: sentinelSeam, machines: sentinelHomes } });
      break;
    case "login": {
      // The World's drawing, for the moment this computer is in it.
      const loginMap = new MachineMap({ words: english ? enMachines : zhMachines, storageKey: MAP_ORDER, still: true, event: () => {} });
      let marked: string | null = null;
      const drawLoginMap = () => {
        const world = store.world.peek();
        if (world) loginMap.update(stillScene(world, marked ? `m:${marked}` : null));
      };
      const stopLoginMap = effect(() => {
        store.world.get();
        drawLoginMap();
      });
      page = new LoginPage({
        store,
        words: english ? enLogin : zhLogin,
        ...(next.way ? { way: next.way } : {}),
        head,
        device: asDevice,
        setup: asDevice ? undefined : () => (live ? setupState() : Promise.reject(new Error("specimen"))),
        place: asDevice ? undefined : () => (live ? placeFastctx() : notLive()),
        login: (hub, account, password, register, name) => (live ? login(hub, account, password, register, name) : notLive()),
        loginToken: (token, name) => (live ? loginToken(token, name) : specimenJoin(token)),
        peekToken: asDevice ? undefined : (token) => (live ? peekToken(token) : specimenPeek(token)),
        remove: asDevice || !removal ? undefined : () => removal.open(true),
        openAccount: asDevice ? undefined : (hub, account, password, register, name) => (live ? openAccount(hub, account, password, register, name) : notLive()),
        addresses: asDevice ? undefined : () => (live ? hereAddresses() : Promise.resolve(specimenAddresses)),
        // A specimen pretends, so the page after it can be looked at.
        create: asDevice
          ? undefined
          : (world, admin, password, reach, register, name) =>
              live ? createWorld(world, admin, password, reach, register, name) : Promise.resolve({ hub: world, name: name ?? "desktop", account: admin, kept: { kind: "installed" as const } }),
        // A World made on a server over SSH: this computer signs in as its
        // administrator, and the server joins with a code made for it.
        remote: asDevice ? undefined : live ? (onComputer() ? remoteSeams : undefined) : specimenRemote(),
        invite: (login) => tokens.open(login),
        // A phone reads the token off the computer's screen; a specimen
        // shows the button when asked as a phone.
        scan: canScan() ? scanToken : !live && params.has("phone") ? async () => "FX5SPECIMEN" : undefined,
        deviceName: !asDevice ? undefined : live ? () => phoneSeam.status().then((status) => status.deviceName) : async () => "REDMI K80 Ultra",
        keep: asDevice ? undefined : () => (live ? keepHere() : notLive()),
        nameProblem: live ? nameProblem : specimenNameProblem,
        drawing: {
          element: loginMap.element,
          mark: (machine) => {
            marked = machine;
            drawLoginMap();
          },
          stop: () => {
            stopLoginMap();
            loginMap.dispose();
          },
        },
        enter() {
          welcomed.now();
          replacePlace(asDevice && !deviceFeatures.conversations.peek() ? link({ kind: "feature", feature: "conversations" }) : link({ kind: "home" }));
        },
      });
      break;
    }
    case "account":
      page = new MePage({
        store,
        words: english ? enMe : zhMe,
        machines: english ? enMachines : zhMachines,
        head,
        when,
        short: (until, now) => (english ? enAccess : zhAccess).moment.short(until, now),
        change,
        confirm: accountConfirm,
        say: (text, options) => talkToast.say(text, options),
      });
      break;
    case "file":
      page = new ReaderPage({
        words: readerWords[english ? "en" : "zh"],
        machine: next.machine,
        path: next.path,
        tail: next.tail,
        head,
        read: (machine, path, range, held) => (live ? readText(machine, path, range, held) : specimenRead(machine, path, range)),
        readBytes: (machine, path, held) => (live ? readBytes(machine, path, held) : notLive()),
        ago: (at) => homeWords.ago(Date.now() - at.getTime()),
      });
      break;
    case "data":
      page = new DataPage({
        words: english ? enData : zhData,
        head,
        data: live ? (onComputer() && !asDevice ? data : undefined) : params.has("phone") ? undefined : specimenData,
        cache: live ? { size: async () => (await keptUsage()).bytes, clear: asDevice ? keptClear : async () => { await data({ do: "clear_cache" }); } } : specimenCache,
        confirm: accountConfirm,
        now: () => new Date(),
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
          keepLanguage(chosen);
          const url = new URL(location.href);
          url.searchParams.delete("lang");
          if (chosen === "en") url.searchParams.set("lang", "en");
          // The device keeps it too, so the TUI speaks it.
          void (live ? setLanguage(chosen).catch((error: unknown) => console.error("keeping the language chosen", error)) : Promise.resolve()).finally(() => location.replace(url));
        },
        ago: (at, now) => homeWords.ago(now.getTime() - at.getTime()),
        renaming: {
          words: english ? enMachinePage : zhMachinePage,
          nameSaid: english ? enNameSaid : zhNameSaid,
          check: (text) => (live ? nameProblem("machine", text) : specimenNameProblem("machine", text)),
          here: async (to) => {
            if (live) return renameHere(to);
            const renamed = specimenRenameHere(store.world.peek(), to);
            if (renamed) store.world.set(renamed);
          },
        },
        phone: live ? (onPhone() ? phoneSeam : undefined) : params.has("phone") ? specimenPhone : undefined,
        updates: {
          updates: appUpdates,
          words: updateWords,
          view: english ? enUpdateView : zhUpdateView,
          open: () => updateCard.open(),
        },
        notifyChanged: (on) => {
          if (live && onPhone()) void configurePhone(presenceWords, on).catch((error: unknown) => console.error("telling the phone its calls", error));
        },
        computer: live
          ? onComputer()
            ? {
                setup: setupState,
                keep: keepHere,
                removing: { plan: removalPlan, run: removeHere, finish: removeApp },
                removal,
                dataSize: async () => {
                  const reply = await data({ do: "list" });
                  return reply.data === "listed" ? totalOf(reply) : 0;
                },
                proxy: { get: () => hubProxy(null, false), set: (url) => hubProxy(url, true) },
                renewing: {
                  state: renewalState,
                  // The program the interface carries is put in place,
                  // then the node is asked to hand over to it.
                  renew: async () => {
                    await placeFastctx();
                    await renewHere();
                  },
                },
              }
            : undefined
          : { ...specimenComputer(), removal },
      });
      break;
  }
  shell.show(next, page);
  shownPage = page;
  setPageBack(!inner(next) ? null : came ? () => history.back() : () => { location.hash = link(parent); });
}

/** The one page of a managed machine. */
function managedPage(): Page {
  return new ManagedPage({
    store,
    words: english ? enManaged : zhManaged,
    when: (at, now) => when(at, now),
    // Joining again starts from this computer off the network it was
    // taken off: its link there ended, and a code brings it in anew.
    rejoin: () => {
      const found = managedOf(store.world.peek()?.accounts);
      const leaving = found ? (live ? accountChange(found.hub.name, found.me.home ?? "", { change: "logout" }) : Promise.resolve(specimenLeaveManaged())) : Promise.resolve();
      void leaving
        .catch((error: unknown) => console.error("leaving the network this computer was taken off", error))
        .then(() => store.refresh())
        .then(() => {
          location.hash = link({ kind: "login", way: "code" });
        });
    },
    remove: removal ? () => removal.open() : undefined,
    // A managed machine no one signs in to is connected by its startup
    // service alone; the specimen shows it missing with `?managed=unkept`.
    service: live
      ? onComputer()
        ? { kept: async () => (await setupState()).service, keep: keepHere }
        : undefined
      : { kept: async () => params.get("managed") !== "unkept", keep: async () => ({ kind: "installed" as const }) },
  });
}

/** In the specimen, a computer that leaves the network it was a managed
 *  machine on is in none after. */
function specimenLeaveManaged(): void {
  const world = store.world.peek();
  if (!world) return;
  const hubs = world.accounts.hubs.filter((hub) => !managedOf({ ...world.accounts, hubs: [hub] }));
  store.world.set({ ...world, accounts: { ...world.accounts, hubs } });
}

/** In the specimen, joining by a code for managed machines makes this
 *  computer one, so the page after it can be looked at. */
async function specimenJoin(token: string) {
  const peeked = await specimenPeek(token);
  const world = store.world.peek();
  if (!peeked?.managed || peeked.refused || !world) return notLive();
  store.world.set(specimenAsManaged(world));
  return { hub: peeked.hub, name: world.here, account: peeked.account, kept: { kind: "installed" as const } };
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
    // `?s=journal&memory`: a decision of the project, its versions beside it.
    case "journal":
      return { kind: "memory", town: "mark50", name: "定点数格式" };
    case "page":
      return { kind: "page", town: "mark50", page: "tuning", version: null };
    case "machines":
      return { kind: "machines", focus: "laptop" };
    case "machine":
      return { kind: "machine", name: params.get("m") ?? "desktop" };
    case "invite":
      return { kind: "machines" };
    case "welcome":
    case "login":
      return { kind: "login" };
    case "account":
      return { kind: "account" };
    case "settings":
      return { kind: "settings" };
    case "data":
      return { kind: "data" };
    case "codex":
      return { kind: "talk", address: { resident: "codex-7f3a", machine: "laptop" } };
    case "asks":
      return { kind: "talk", address: { resident: "codex-a51c", machine: "laptop" } };
    case "stream":
      return { kind: "talk", address: { resident: "stream-1", machine: "laptop" } };
    case "steps":
      return { kind: "talk", address: { resident: "claude-5teps", machine: "laptop" } };
    case "trips":
      return { kind: "talk", address: { resident: "trips-1", machine: "desktop" } };
    case "deeds":
      return { kind: "talk", address: { resident: "deeds-1", machine: "laptop" } };
    case "tour":
      return { kind: "talk", address: { resident: "claude-t0ur", machine: "laptop" } };
    case "voice":
      return { kind: "talk", address: { resident: "claude-v01c", machine: "laptop" } };
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
      { group: paletteWords.actions, label: shellWords.newTalk, note: "Ctrl N", run: () => beginTalk(place.kind === "project" || place.kind === "new" ? place.town : null) },
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
          label: titleOf(r) ?? someoneSaid(r),
          note: [titleOf(r) ? someoneSaid(r) : null, r.town].filter(Boolean).join(" "),
          run: go({ kind: "talk", address: { resident: r.about.id, machine: r.about.machine } }),
        })),
      ...(inNoNetwork(world) ? [] : machinesOf(world)).map((machine) => ({
        group: paletteWords.machines,
        label: machine.written,
        note: machine.here || machine.online ? "" : shellWords.offline,
        run: go({ kind: "machine", name: machine.written }),
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
  // A computer is useful on its own: it opens on its agents and is never
  // sent to join a network (`world-gui.md`, 账号、机器、共享与协助). Only
  // when fastctx is not on it yet is the way on to put it there.
  let decided = false;
  effect(() => {
    const trouble = store.trouble.get();
    if (!trouble || decided || store.world.peek()) return;
    decided = true;
    void setupState()
      .then((setup) => {
        if (!setup.installed && (place.kind === "home" || place.kind === "agents")) replacePlace(link({ kind: "login" }));
      })
      .catch(() => {});
  });
}
// The front page comes and goes with the workbench: the place shown follows.
effect(() => {
  const talking = deviceFeatures.conversations.get();
  if (!talking && place.kind === "home") show({ kind: "agents", machine: null });
});

// Everything that broke is tried again at once when the window comes to the
// front or the network is back (`world-network.md`, 断开不是结束).
if (live) {
  const now = () => void retryNow().catch((error: unknown) => console.error("trying again now", error));
  addEventListener("focus", now);
  addEventListener("online", now);
}

// The workbench as this device keeps it: a computer's node, shared with its TUI.
if (live) {
  deviceFeatures.attach({
    read: async () => (await readFeatures()).conversations,
    write: async (on) => void (await setFeature("conversations", on)),
    follow: (heard) =>
      void watchFeatures((piece) => {
        if (piece.kind === "features") heard(piece.features.conversations);
      }).catch((error: unknown) => console.error("following this device's features", error)),
  });
}

installPens();
installPresses();
installHints();
installSheetDrag();
installPointerHints();
installRules();
installAccess();
installBack();
// A specimen shows the window's frame when asked, to be looked at.
installFrame(shellWords.frame, !live && params.has("frame"));
nameCodeCopy(words.copyCode);
addEventListener("hashchange", () => show(placeOf(location.hash)));
applyTheme(() => {
  void redrawDiagrams(shell.element);
  openPage?.contextChanged();
});
show(specimenPlace() ?? placeOf(location.hash));
// A device in no network any more, however it left (it logged out, the
// network was dissolved, its hub went away), has only the way in to show,
// as when it first opened (`world-gui.md`).
let deviceIn = loginsOf(store.world.peek()?.accounts).length > 0;
effect(() => {
  const world = store.world.get();
  if (!world) return;
  const inAny = loginsOf(world.accounts).length > 0;
  const left = deviceIn && !inAny;
  deviceIn = inAny;
  if (live && asDevice && left && place.kind !== "login" && place.kind !== "data") replacePlace(link({ kind: "login" }));
});
// Becoming a managed machine, or no longer being one, changes the whole
// window (`world-gui.md`, 受管机器上的界面).
let shownAlone = !!managedOf(store.world.peek()?.accounts);
effect(() => {
  const alone = !!managedOf(store.world.get()?.accounts);
  if (alone === shownAlone) return;
  shownAlone = alone;
  show(place);
});
// A device logged in nowhere has nothing to show but the way in.
if (live) {
  void connectionState().then((state) => {
    asDevice = state.mode === "device";
    // The data page shows a device only its cache, which it knows now.
    if (place.kind === "data") show(place);
    if (asDevice && !state.device?.hubs.length && (place.kind === "home" || place.kind === "agents")) replacePlace(link({ kind: "login" }));
    else if (place.kind === "login") show(place);
  });
}
if (!live && params.get("s") === "new") beginTalk(params.get("town"));
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
  someone: (r) => someoneSaid(r),
  stop: (address) => stop(address),
  // An answer from a notification goes in once, however long the way is
  // closed (`world-network.md`, 断开不是结束).
  say: (address, text) => {
    const id = crypto.randomUUID();
    return untilAnswered(() => talk(address, text, [], null, id), () => {}, address.machine);
  },
});

// A network this computer or phone was on, dissolved, is said once.
noticeDissolved({
  store,
  words: english ? enMachines : zhMachines,
  close: (english ? enAccess : zhAccess).close,
  phone: asDevice,
  seen: async () => {
    if (live) return seenDissolved();
    const world = store.world.peek();
    if (world) store.world.set({ ...world, accounts: { ...world.accounts, dissolved: [] } });
  },
});

/** A picture's bytes, as a blob the viewer shows. */
function blobOf(picture: Thumbnail): Blob {
  return new Blob([Uint8Array.from(atob(picture.base64), (char) => char.charCodeAt(0))], { type: picture.mime });
}

/** Whether a save went through: a change of mind is not a failure. */
function savedOf(saved: Saved): boolean {
  if (saved.kind === "failed") throw new Error(saved.reason);
  return saved.kind === "saved";
}
