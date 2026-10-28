// Beginning a conversation (`world-gui.md`, 界面的结构): a page of its own,
// for a project's new conversation and for one outside any, each keeping
// what was being written there until it is sent, however often the user
// goes elsewhere and comes back.
//
// Its input stands where a conversation's does, the same box, so the
// conversation it begins grows above the words without the page moving.
// Above them, small labels say what the words go to, and each opens what it
// can be: which agent, as one of its logins, chosen among those the user
// holds, grouped as they hold them (on their own machines, on machines of
// others they may work on, assisting them from another account), each with who
// is logged in and how much of its usage is spent, as its machine says; the
// project; and the machines and folders the conversation is about, among
// those its account may use, told to the agent as where its work is and
// never a limit (`focus-panel.ts`). In a project those start as its
// machines and the places its record lists on them. The model and how hard
// it thinks are the conversation's own chip and panel. Above the input, the
// chosen agent's mark and the place's name; nothing more.
//
// An agent assisting the user runs on a machine it may not touch: each of its
// commands names one of the user's machines, and the files attached for
// it, other than images, go to a machine of the user's without asking
// (`fileHome`). Which agents may call which models is the model
// collaboration feature's, never a conversation's.
//
// Every machine is asked what agents it has each time the page is shown.
// Files wait in the input while the agent and project are still being
// chosen, and go when the conversation is begun: the first makes a place on
// the agent's machine for the agent about to be asked in, the rest join it
// there, and the agent starts in that place with all of them.

import { AttachTray, type AttachSource, type AttachWords } from "../conversation/attach";
import { layerOpened } from "./back";
import { glyph } from "../conversation/glyphs";
import { ModelPanel, type ModelPanelWords } from "../conversation/model-panel";
import { FocusPanel, focusSummary, type FocusPlace, type FocusServers } from "./focus-panel";
import { noFocus, type Focus, type Folders } from "../world/focus";
import { escape } from "../conversation/prose";
import { Starting, type Seed } from "../conversation/starting";
import { departureOf } from "../conversation/flight";
import type { Words } from "../words";
import type { HubAccount, Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentFacts, AgentModel, WorldAgents } from "../world/agents";
import type { Attachment, FilePlace, UploadTarget } from "../world/attachments";
import type { WorldNow } from "../world/bridge";
import type { Offers } from "../world/control";
import type { Address } from "../world/events";
import { accountOf, homeChoice, homeLabel, homesOf, installed, listed, loggedOut, type HomeWords } from "./agent-homes";
import { agentKinds, agentPicker, foldInPicker, type PickerKind, type PickerMachine, type PickerRow } from "./agent-picker";
import { resizeUpward } from "./resize-upward";
import { keepChoice, keptEffort, keptModel } from "./model-memory";
import type { ProfileDefaults } from "./profile-defaults";
import { focusPlace } from "./focus-places";
import type { ConsentHome } from "./claude-consent";
import {
  assistantsOf,
  fileHome,
  filePlaces,
  groupsOf,
  loginKey,
  loginLabel,
  machineNamed,
  machinesOf,
  machineWritten,
  openersOn,
  ownerOf,
  owns,
  sameLogin,
  workPlaces,
  type Assistant,
  type Machine,
} from "./reach";
import { DraftFit } from "../conversation/fit-draft";
import { whenNear } from "./read-on";
import { Steady } from "./steady";
import type { Store } from "./store";
import type { FocusWords, NewTalkWords } from "./words";
import { composing } from "../typing";

/** How many of the table of agents' rows are drawn at first, and how many
 *  more each time the end of those comes near. */
const PICKER_ROWS = 120;

export interface Begun {
  agent: string;
  machine: string;
  town: string | null;
  text: string;
  /** The program's own model and effort names; its defaults where null. */
  model: string | null;
  effort: string | null;
  /** What was attached, already on the machine, for the agent to start with. */
  attachments: Attachment[];
  /** The account it works for, where the user chose among several or the
   *  agent assists one. */
  account: HubAccount | null;
  /** The machines and folders it is pointed at. */
  focus: Focus;
  /** The agent's login on the machine it lives on, as its home's
   *  directory; null for the default one. */
  home: string | null;
  /** Where its files other than images go, for an agent assisting the user. */
  filesOn: string | null;
  /** The account's MCP servers it starts without. */
  mcpOff: string[];
}

export interface NewTalkOptions {
  /** The MCP servers of the account a conversation would work for, as its
   *  switches list them. */
  servers?: ((login: () => Login | null) => Omit<FocusServers, "off" | "changed">) | undefined;
  /** The model and effort the preferences of the account a conversation
   *  would work for name, before the device's own memory. */
  defaults?: ProfileDefaults | undefined;
  store: Store;
  words: NewTalkWords;
  focusWords: FocusWords;
  homeWords: HomeWords;
  modelWords: ModelPanelWords;
  /** The head every page has. */
  head(title: string): string;
  /** The project the page begins conversations in; null outside any. */
  town: string | null;
  /** What the machines' own programs say of the agents on them: one, or
   *  with null every machine reached. */
  agents(machine: string | null): Promise<WorldAgents>;
  /** The folders in a folder of a machine, for pointing it at some. */
  folders(machine: string, path: string | null): Promise<Folders>;
  begin(begun: Begun): Promise<Address>;
  /** The conversation began: its page opens, with the first words as they
   *  were shown here. */
  opened(address: Address, seed: Seed | null): void;
  /** The conversation's own words, and this device's name, for showing it
   *  begin (`conversation/starting.ts`). */
  starting?: { words: Words; here(): string };
  /** Asks the user to let Claude work without asking in one of its logins
   *  on a machine, where it has not yet; true once Claude Code there
   *  records it. */
  allowClaude?(machine: string, go: string, home: ConsentHome): Promise<boolean>;
  /** Attaching files to the first words, where the World can carry them. */
  attach?: {
    words: AttachWords;
    /** Carries one file to a machine for the conversation about to begin;
     *  `place` is where a file other than an image goes, for an agent of
     *  another's assisting the user. */
    upload(machine: string, target: UploadTarget, source: AttachSource, progress: (sent: number) => void, place: FilePlace | null): Promise<Attachment>;
    /** Files dropped onto the window, by path, where the shell gives
     *  them so; returns how to stop listening. */
    drops?(on: { over(over: boolean): void; dropped(paths: string[]): void }): () => void;
  };
}

interface Choice {
  value: string;
  label: string;
  note?: string | undefined;
  disabled?: boolean;
}

/** How the user holds a machine: their own, another's they may work on,
 *  or another's with only an agent's login assisting them. */
type Hold = "mine" | "others" | "assist";

/** One agent's login a conversation may begin with. */
interface Option {
  key: string;
  machine: string;
  written: string;
  agent: string;
  /** As the World is told it: the home's directory, or null for the
   *  default one. */
  home: string | null;
  /** Assisting one of the user's logins. */
  assistant: Assistant | null;
}

/** One machine in the picker, with the logins on it. */
interface Line {
  hold: Hold;
  machine: string;
  written: string;
  here: boolean;
  online: boolean | null;
  owner: string | null;
  /** Another's machine shared with one of the user's logins, rather than
   *  a member's the super administrator may use. */
  shared: boolean;
  options: Option[];
}

/** What a label above the input opens. */
type Pop = "agent" | "town" | "focus";

const agentsKnown = ["codex", "claude"];
const programName = (agent: string) => (agent === "codex" ? "Codex" : agent === "claude" ? "Claude" : agent);
const markOf = (agent: string) => (agent === "claude" ? "claude" : "codex");
const coarse = () => matchMedia("(pointer: coarse)").matches;

/** What is known of the agents on one machine. */
type Asked = { state: "asking" } | { state: "known"; facts: AgentFacts[] } | { state: "unknown"; why: string } | { state: "offline" };

function remembered(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function remember(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Not kept: the next page starts from the usual choice.
  }
}

/** The machines and people the user opened or closed in the picker, as
 *  they left them last. */
function rememberedFolds(): Map<string, boolean> {
  try {
    const kept = JSON.parse(remembered("pickerFolds") ?? "[]") as unknown;
    return new Map(Array.isArray(kept) ? kept.filter((one): one is [string, boolean] => Array.isArray(one) && typeof one[0] === "string" && typeof one[1] === "boolean") : []);
  } catch {
    return new Map();
  }
}

/** A login an agent can start in: its program is there and it is not
 *  known to be logged out. */
function usable(facts: AgentFacts): boolean {
  return installed(facts) && !loggedOut(facts);
}

export class NewTalk {
  readonly element: HTMLElement;
  private readonly form: HTMLFormElement;
  private readonly input: HTMLTextAreaElement;
  private readonly note: HTMLElement;
  private readonly start: HTMLButtonElement;
  private readonly context: HTMLElement;
  private readonly pop: HTMLElement;
  private readonly chip: HTMLButtonElement;
  private readonly models: ModelPanel;
  /** The login chosen, by its key; the one used last where none is. */
  private pick: string | null = remembered("pick");
  /** A machine whose logins come first, when the page was opened from it. */
  private machine: string | null = null;
  private town: string | null;
  private model: string | null = null;
  private effort: string | null = null;
  /** The model and effort were chosen on this page, not taken as they
   *  were kept. */
  private chosenHere = false;
  /** The account chosen, as `hub/account`. */
  private account: string | null = remembered("account");
  /** The machines and folders it is pointed at. */
  private focus: Focus = noFocus();
  /** The account's MCP servers switched off for the conversation begun. */
  private mcpOff: string[] = [];
  /** The project the machines and folders were last set from; changed by
   *  the user since, they are theirs. */
  private focusFrom: { town: string | null } | null = null;
  private focusTouched = false;
  private readonly focusPanel: FocusPanel;
  /** Where the files of an assisting agent go, as written. */
  private filesOn: string | null = null;
  private asked = new Map<string, Asked>();
  /** Every machine is being asked about its agents. */
  private askingAll = false;
  /** The label whose choices are open. */
  private opened: Pop | null = null;
  private busy = false;
  private tray: AttachTray | null = null;
  /** The place on a machine the files sent so far went to, and for which
   *  machine, agent and project; another choice needs another place. */
  private draft: { resident: string; for: string } | null = null;
  private stopDrops: (() => void) | null = null;
  /** The mark drawn last, so it is drawn again only for another agent. */
  private marked = "";
  /** Where the words being written are kept between visits. */
  private readonly kept: string;
  /** The conversation being begun, shown in place of the stage. */
  private readonly starting: Starting | null = null;

  constructor(private readonly options: NewTalkOptions) {
    const { words } = options;
    this.town = options.town;
    this.kept = `new-draft:${options.town ?? ""}`;
    this.element = document.createElement("div");
    this.element.className = "new-talk-page";
    this.element.innerHTML = `
      ${options.head("")}
      <div class="new-talk-stage">
        <div class="new-talk-mark" aria-hidden="true"></div>
        <h1 class="new-talk-title"></h1>
      </div>
      <footer class="composer-dock new-talk-dock">
        <form class="composer new-talk-composer">
          <div class="composer-menus"><div class="talk-pop" role="dialog" hidden></div></div>
          <div class="talk-context"></div>
          <div class="composer-tray"></div>
          <textarea rows="1" enterkeyhint="send"></textarea>
          <div class="composer-row">
            <div class="composer-tools"><button class="icon-button composer-attach" type="button" hidden></button><input class="composer-pick" type="file" multiple hidden><button class="icon-button composer-photo" type="button" hidden></button><input class="composer-shot" type="file" accept="image/*" capture="environment" hidden><button class="composer-chip" type="button" hidden></button></div>
            <span class="composer-note" aria-live="polite"></span>
            <button class="composer-action" type="submit" aria-label="${escape(words.start)}" title="${escape(words.start)}">${glyph("send", 18)}</button>
          </div>
        </form>
      </footer>`;
    if (options.starting) {
      this.starting = new Starting(options.starting.words, options.starting.here, () => departureOf(this.input));
      this.element.querySelector(".new-talk-stage")!.after(this.starting.element);
    }
    this.form = this.element.querySelector("form")!;
    this.input = this.element.querySelector("textarea")!;
    this.note = this.element.querySelector<HTMLElement>(".composer-note")!;
    this.start = this.element.querySelector<HTMLButtonElement>(".composer-action")!;
    this.context = this.element.querySelector<HTMLElement>(".talk-context")!;
    this.pop = this.element.querySelector<HTMLElement>(".talk-pop")!;
    this.chip = this.element.querySelector<HTMLButtonElement>(".composer-chip")!;
    this.models = new ModelPanel(this.chip, options.modelWords, words.english, {
      model: async (model, effort) => {
        this.model = model;
        this.effort = effort;
        this.chosenHere = true;
        // Chosen here, it is what the next new conversation begins with.
        const agent = this.chosenOption()?.agent;
        if (agent) keepChoice(agent, model, effort);
        this.drawModel();
      },
      tier: async () => {},
    });
    this.element.querySelector(".composer-menus")!.append(this.models.element);
    // The account's preferences, read once asked, may name another model.
    options.defaults?.watch(() => this.drawModel());
    this.focusPanel = new FocusPanel(
      {
        words: options.focusWords,
        places: () => this.focusPlaces(),
        folders: options.folders,
        changed: (focus) => {
          this.focus = focus;
          this.focusTouched = true;
          this.draw();
        },
        marks: (machine) => this.projectMarks(machine),
        resize: (change) => resizeUpward(this.pop, this.popSteady, change),
        servers: options.servers
          ? {
              ...options.servers(() => this.loginFor(this.options.store.world.peek(), this.chosenOption())),
              off: () => this.mcpOff,
              changed: (off) => {
                this.mcpOff = off;
              },
            }
          : undefined,
      },
      this.focus,
    );
    this.input.value = remembered(this.kept) ?? "";
    this.element.addEventListener("click", (event) => this.click(event));
    // A choice open over the input goes away with a press anywhere else,
    // or with Escape.
    document.addEventListener("pointerdown", (event) => {
      if (!this.opened || !this.element.isConnected) return;
      const target = event.target as HTMLElement;
      // A window opened from the choices, such as a machine's folders, is
      // part of them.
      if (!this.pop.contains(target) && !target.closest("[data-pop]") && !target.closest("dialog")) this.choose(null);
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.opened) {
        event.stopPropagation();
        // The labels are drawn anew as the choices close: the one to go
        // back to is found after.
        const opened = this.opened;
        this.choose(null);
        this.context.querySelector<HTMLElement>(`[data-pop="${opened}"]`)?.focus();
      }
    });
    this.form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    this.input.addEventListener("input", () => {
      remember(this.kept, this.input.value || null);
      this.fit();
      this.drawStart();
    });
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey && !composing(event) && !coarse()) {
        event.preventDefault();
        void this.submit();
      }
    });
    if (options.attach) this.attaching(options.attach);
    this.draw();
  }

  /** The page's name: the project's, or a new conversation's. */
  get title(): string {
    const { words, store } = this.options;
    const town = this.options.town ? store.world.peek()?.towns.find((one) => one.town === this.options.town) : null;
    return town ? `${words.title} ${town.record.name}` : words.title;
  }

  /** The page is shown again: what each machine has is asked again, as a
   *  program may have been installed, logged in or updated since; `pick`
   *  is a login to begin with, `machine` one whose logins come first. */
  show(pick: string | null = null, machine: string | null = null): void {
    if (pick) this.pick = pick;
    if (pick || machine) this.machine = machine;
    this.note.textContent = "";
    this.asked.clear();
    this.askAll();
    this.stopDrops?.();
    this.stopDrops = this.listen();
    this.draw();
    queueMicrotask(() => {
      this.fit();
      if (!coarse()) this.input.focus({ preventScroll: true });
    });
  }

  /** The page is left: it stops listening for files dropped on the window;
   *  what was written and attached stays for when it is shown again. */
  leave(): void {
    // A conversation begun from here opened on its own page; the next is
    // begun on an empty stage.
    if (!this.busy) this.unstage();
    this.stopDrops?.();
    this.stopDrops = null;
    this.choose(null);
    this.models.hide();
    this.pickerUnwatch();
  }

  /** The stage as it is before anything is begun. */
  private unstage(): void {
    this.starting?.clear();
    this.element.classList.remove("begun");
  }

  private fitter: DraftFit | null = null;
  /** The choices open over the input keep the size they opened at, as
   *  more answers come only growing (`steady.ts`). */
  private popSteady: Steady | null = null;
  /** What narrows the table of agents, while it is open. */
  private narrowed = "";
  /** Which agent's logins the picker shows, and the machines and people
   *  opened or closed in it. */
  private kind: PickerKind = remembered("agent") === "claude" ? "claude" : "codex";
  private readonly pickerFolds = rememberedFolds();
  /** The table as last drawn, so it is drawn again only when it differs. */
  private agentsDrawn = "";
  /** How many of the table's rows are drawn, how many each person's opens
   *  onto as last drawn, and what stops watching the end of those coming
   *  near. */
  private pickerReach = PICKER_ROWS;
  private pickerInside = new Map<string, number>();
  private pickerUnwatch = () => {};
  /** The reader chose an agent: the table changes within the panel's
   *  size, and the panel does not grow for it. */
  private keepSize = false;

  private fit(): void {
    this.fitter ??= new DraftFit(this.input);
    this.fitter.fit();
  }

  /** Files come with the first words: picked with the paperclip, pasted,
   *  or dropped onto the page. */
  private attaching(attach: NonNullable<NewTalkOptions["attach"]>): void {
    const tray = new AttachTray(
      attach.words,
      {
        upload: async (source, _name, progress) => {
          const option = this.chosenOption()!;
          const chosen = this.chosen();
          const target: UploadTarget =
            this.draft?.for === chosen
              ? { kind: "draft", resident: this.draft.resident, town: this.town }
              : { kind: "new", agent: option.agent, town: this.town };
          const login = option.assistant?.login;
          const place = login && this.filesOn ? { account: { hub: login.hub, account: login.account }, filesOn: this.filesOn } : null;
          const done = await attach.upload(option.written, target, source, progress, place);
          this.draft = { resident: done.owner.resident, for: chosen };
          return done;
        },
      },
      () => {
        // The sending device fixes the location before the conversation starts.
        this.placeFiles();
        this.drawStart();
      },
      true,
    );
    tray.onlyImages(() => this.imagesOnly(), (why) => this.refuseFile(why));
    this.tray = tray;
    this.element.querySelector(".composer-tray")!.replaceWith(tray.element);
    const button = this.element.querySelector<HTMLButtonElement>(".composer-attach")!;
    const pick = this.element.querySelector<HTMLInputElement>(".composer-pick")!;
    button.hidden = false;
    button.innerHTML = glyph("attach", 18);
    button.setAttribute("aria-label", attach.words.attach);
    button.title = attach.words.attach;
    button.addEventListener("click", () => pick.click());
    pick.addEventListener("change", () => {
      if (pick.files) tray.addFiles(pick.files);
      pick.value = "";
      this.input.focus();
    });
    // As in a conversation: the camera on a phone, nowhere else.
    if (coarse()) {
      const photo = this.element.querySelector<HTMLButtonElement>(".composer-photo")!;
      const shot = this.element.querySelector<HTMLInputElement>(".composer-shot")!;
      photo.hidden = false;
      photo.innerHTML = glyph("camera", 18);
      photo.setAttribute("aria-label", attach.words.photo);
      photo.title = attach.words.photo;
      photo.addEventListener("click", () => shot.click());
      shot.addEventListener("change", () => {
        if (shot.files) tray.addFiles(shot.files);
        shot.value = "";
        this.input.focus();
      });
    }
    this.input.addEventListener("paste", (event) => {
      const files = [...(event.clipboardData?.files ?? [])];
      if (!files.length) return;
      // Pasted files are attached; pasted words still go into the words.
      if (!event.clipboardData?.getData("text/plain")) event.preventDefault();
      tray.addFiles(files);
    });
    const veil = document.createElement("div");
    veil.className = "drop-veil";
    veil.hidden = true;
    veil.innerHTML = `<div class="drop-card">${glyph("attach", 22)}<span>${escape(this.options.words.dropHere)}</span></div>`;
    this.element.append(veil);
    // In a browser, files dragged over the page; in the shell, the window
    // takes the drop and gives their paths, heard while the page is shown.
    let depth = 0;
    this.element.addEventListener("dragenter", (event) => {
      if (!event.dataTransfer?.types.includes("Files")) return;
      depth += 1;
      veil.hidden = false;
    });
    this.element.addEventListener("dragleave", () => {
      depth = Math.max(0, depth - 1);
      if (!depth) veil.hidden = true;
    });
    this.element.addEventListener("dragover", (event) => {
      if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
    });
    this.element.addEventListener("drop", (event) => {
      depth = 0;
      veil.hidden = true;
      if (!event.dataTransfer?.files.length) return;
      event.preventDefault();
      if (!this.busy) tray.addFiles(event.dataTransfer.files);
    });
    this.listen = () =>
      attach.drops?.({
        over: (over) => {
          if (this.element.isConnected) veil.hidden = !over;
        },
        dropped: (paths) => {
          veil.hidden = true;
          if (this.element.isConnected && !this.busy) tray.addPaths(paths);
        },
      }) ?? null;
  }

  private listen: () => (() => void) | null = () => null;

  /** The machine, account, agent and project the files go for. */
  private chosen(): string {
    const option = this.chosenOption();
    return JSON.stringify([option?.machine ?? null, this.loginFor(this.options.store.world.peek(), option)?.account ?? null, option?.agent ?? null, this.town]);
  }

  /** What a machine said of its agents, once it has. */
  private known(machine: string | null): AgentFacts[] | null {
    const asked = machine ? this.asked.get(machine) : undefined;
    return asked?.state === "known" ? asked.facts : null;
  }

  /** The login an option names, as its machine says it, once it has. */
  private factsOf(option: Option | null): AgentFacts | null {
    if (!option) return null;
    return homesOf(this.known(option.machine) ?? [], option.agent).find((facts) => homeChoice(facts) === option.home) ?? null;
  }

  /** Asks every machine reached what agents it has, at once. */
  private askAll(): void {
    this.askingAll = true;
    this.options
      .agents(null)
      .then((world) => {
        for (const answer of world.machines) {
          if (answer.kind === "answered") this.asked.set(answer.reply.machine, { state: "known", facts: answer.reply.agents });
          else if (answer.kind === "offline") this.asked.set(answer.machine, { state: "offline" });
          else this.asked.set(answer.machine, { state: "unknown", why: answer.reason });
        }
      })
      .catch((error: unknown) => {
        for (const line of this.lines(this.options.store.world.peek())) {
          if (!this.asked.has(line.machine)) this.asked.set(line.machine, { state: "unknown", why: String(error) });
        }
      })
      .finally(() => {
        this.askingAll = false;
        this.draw();
      });
  }

  /** Every machine with agents the user may begin a conversation with,
   *  as they hold it, and the logins on each as its machine says them;
   *  where it has not said, a login of each agent, as unknown is not no. */
  private lines(world: WorldNow | null): Line[] {
    const logins = loginsOf(world?.accounts);
    const lines: Line[] = [];
    for (const machine of machinesOf(world)) {
      if (!machine.here && !openersOn(world, machine).length) continue;
      const mine = machine.here || logins.some((login) => owns(machine, login));
      const owner = mine ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null);
      const shared = !mine && logins.some((login) => world?.accounts.hubs.some((hub) => hub.hub === login.hub && hub.shares.some((share) => share.machine === machine.name && share.account === login.account)));
      const facts = this.known(machine.name);
      const options = facts
        ? agentsKnown.flatMap((agent) =>
            homesOf(facts, agent)
              // The World uses only the logins in a machine's agent list.
              .filter((home) => installed(home) && listed(home))
              .map((home) => this.option(machine.name, machine.written, agent, homeChoice(home), null)),
          )
        : agentsKnown.map((agent) => this.option(machine.name, machine.written, agent, null, null));
      lines.push({ hold: mine ? "mine" : "others", machine: machine.name, written: machine.written, here: machine.here, online: machine.here ? true : machine.online, owner, shared, options });
    }
    for (const assistant of assistantsOf(world)) {
      let line = lines.find((one) => one.hold === "assist" && one.written === assistant.written);
      if (!line) {
        line = { hold: "assist", machine: assistant.machine, written: assistant.written, here: false, online: assistant.online, owner: assistant.owner, shared: false, options: [] };
        lines.push(line);
      }
      if (!line.options.some((option) => option.agent === assistant.agent && option.home === assistant.home)) {
        line.options.push(this.option(assistant.machine, assistant.written, assistant.agent, assistant.home, assistant));
      }
    }
    return lines;
  }

  private option(machine: string, written: string, agent: string, home: string | null, assistant: Assistant | null): Option {
    return { key: loginKey(machine, agent, home), machine, written, agent, home, assistant };
  }

  /** The option chosen, as it stands now. */
  private chosenOption(lines: Line[] = this.lines(this.options.store.world.peek())): Option | null {
    return lines.flatMap((line) => line.options).find((option) => option.key === this.pick) ?? null;
  }

  /** Whether an option can begin a conversation: its machine answers and
   *  its login is usable, or its machine has not said otherwise. */
  private startable(option: Option, line: Line): boolean {
    if (line.online === false) return false;
    const facts = this.factsOf(option);
    if (facts) return usable(facts);
    return this.known(option.machine) === null;
  }

  /** The logins that may open a conversation with an option's agent, and
   *  the one it works for: the only one, the one picked, or for an
   *  assisting agent the login it assists. */
  private openers(world: WorldNow | null, option: Option | null): { all: Login[]; chosen: Login | null } {
    if (!option) return { all: [], chosen: null };
    if (option.assistant) return { all: [option.assistant.login], chosen: option.assistant.login };
    const lives = this.lives(world, option);
    const all = lives ? openersOn(world, lives) : [];
    const key = (login: Login) => `${login.hub}/${login.account}`;
    const chosen = all.find((login) => key(login) === this.account) ?? all[0] ?? null;
    return { all, chosen };
  }

  private loginFor(world: WorldNow | null, option: Option | null): Login | null {
    return this.openers(world, option).chosen;
  }

  /** The machine an option's agent lives on, where the user may work on
   *  it. */
  private lives(world: WorldNow | null, option: Option | null): Machine | null {
    if (!option || option.assistant) return null;
    return machineNamed(world, option.machine);
  }

  /** The logins of `agent` on `machine` as choices: who is logged in to
   *  each and on what plan. */
  private logins(machine: string | null, agent: string): Choice[] {
    const { words, homeWords } = this.options;
    const facts = this.known(machine);
    const homes = facts ? homesOf(facts, agent).filter(installed) : [];
    return homes.map((home) => {
      const plan = accountOf(home).plan;
      return {
        value: homeChoice(home) ?? "",
        label: homeLabel(home, homeWords),
        note: loggedOut(home) ? words.loggedOut : (plan ?? undefined),
        disabled: loggedOut(home),
      };
    });
  }

  /** The models the chosen login's program lists. */
  private modelsOf(option: Option | null): AgentModel[] {
    const facts = this.factsOf(option);
    return facts?.models.kind === "known" ? facts.models.value : [];
  }

  /** Who is logged in to a login, where that tells it apart: the machine
   *  has several of its agent's logins, or it assists. */
  private who(option: Option, line: Line): string | null {
    const facts = this.factsOf(option);
    if (!facts) return null;
    const several = line.options.filter((other) => other.agent === option.agent).length > 1;
    return several || option.assistant ? (accountOf(facts).email ?? homeLabel(facts, this.options.homeWords)) : null;
  }

  /** One login as a row of the picker: who is logged in; why it cannot
   *  begin, or that Claude still needs the user's consent there. */
  private row(option: Option, line: Line): PickerRow {
    const { words } = this.options;
    const asked = this.asked.get(option.machine);
    const facts = this.factsOf(option);
    let note: string | null = null;
    if (line.online === false) note = words.offline;
    else if (!asked || asked.state === "asking" || (this.askingAll && !facts && asked.state !== "known")) note = words.asking;
    else if (asked.state === "offline") note = words.offline;
    else if (asked.state === "unknown") note = words.unsure;
    else if (!facts) note = option.assistant ? words.assistantGone : words.notInstalled;
    else if (loggedOut(facts)) note = words.loggedOut;
    const caution = !note && facts?.unattended?.kind === "known" && !facts.unattended.value ? words.needsConsent : null;
    return {
      key: option.key,
      agent: option.agent,
      facts,
      who: facts ? (accountOf(facts).email ?? homeLabel(facts, this.options.homeWords)) : null,
      note,
      caution,
      startable: this.startable(option, line),
      chosen: option.key === this.pick,
    };
  }

  /** The agents the user may begin with: the machines as the user holds
   *  them, a row a login, in one table (`./agent-picker`). */
  private picker(lines: Line[]): { machines: PickerMachine[]; html: string } {
    const { words } = this.options;
    const now = new Date();
    const machines: PickerMachine[] = lines.map((line) => {
      const assistant = line.options.find((option) => option.assistant)?.assistant ?? null;
      const notes =
        line.hold === "assist"
          ? [words.assistFrom(line.owner ?? ""), assistant?.until ? words.assistUntil(new Date(assistant.until), now) : ""]
          : [line.here ? words.here : "", line.owner && line.hold !== "mine" ? words.ownedBy(line.owner) : ""];
      const facts = this.machineOf(line);
      const form = facts?.form === "laptop" ? "laptop" : facts?.form === "server" ? "server" : "desktop";
      return {
        hold: line.hold === "others" ? (line.shared ? "shared" : "others") : line.hold,
        // Agents assisting the user are listed as whose, never by their machine.
        written: line.hold === "assist" ? words.assistAgents(line.owner ?? "") : line.written,
        owner: line.owner,
        form,
        online: line.online,
        notes: [...notes, line.online === false ? words.offline : ""].filter(Boolean),
        rows: line.options.map((option) => this.row(option, line)),
      };
    });
    const drawn = agentPicker(machines, this.pickerWords(), { kind: this.kind, open: (key, byDefault) => this.pickerFolds.get(key) ?? byDefault, query: this.narrowed.trim().toLowerCase(), reach: this.pickerReach });
    this.pickerInside = drawn.inside;
    return { machines, html: drawn.html };
  }

  private pickerWords() {
    const { words } = this.options;
    return {
      holds: { mine: words.mine, shared: words.others(true), others: words.others(false), assist: words.assisting },
      agent: words.agent,
      noneOf: words.noneOf,
      noMachine: words.noMachine,
      token: words.token,
      onCredits: words.onCredits,
      reset: words.limitReset,
      personSub: words.personSub,
    };
  }

  /** What is known of a line's computer: its kind, for its icon. */
  private machineOf(line: Line) {
    const machine = machineWritten(this.options.store.world.peek(), line.written);
    return machine && "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
  }

  /** A short list to choose one from, the chosen one marked. */
  private list(kind: Pop, label: string, choices: Choice[], chosen: string): string {
    return `<div class="pop-list" role="radiogroup" aria-label="${escape(label)}">${choices
      .map((choice) => {
        const on = choice.value === chosen;
        return `<button type="button" class="pop-row${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-choice="${kind}" data-value="${escape(choice.value)}"${choice.disabled ? " disabled" : ""}><span class="pop-label">${escape(choice.label)}</span>${choice.note ? `<span class="choice-note">${escape(choice.note)}</span>` : ""}</button>`;
      })
      .join("")}</div>`;
  }

  /** A few answers as pills, the chosen one washed in clay. */
  private pills(kind: "account", label: string, choices: Choice[], chosen: string): string {
    return `<div class="choice-options" role="radiogroup" aria-label="${escape(label)}">${choices
      .map((choice) => {
        const on = choice.value === chosen;
        return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-choice="${kind}" data-value="${escape(choice.value)}"${choice.disabled ? " disabled" : ""}>${escape(choice.label)}${choice.note ? `<span class="choice-note">${escape(choice.note)}</span>` : ""}</button>`;
      })
      .join("")}</div>`;
  }

  /** Opens one label's choices, or with null closes what is open. */
  private choose(pop: Pop | null): void {
    // The machines and folders open at their list, wherever they were left.
    if (this.opened === "focus" && pop !== "focus") this.focusPanel.reset();
    if (pop) {
      this.models.hide();
      layerOpened({ isOpen: () => this.opened === pop && this.element.isConnected, close: () => this.choose(null) });
    }
    const opening = pop !== null && pop !== this.opened;
    // The agents open at the one chosen.
    if (opening && pop === "agent") {
      const chosen = this.chosenOption();
      if (chosen) this.kind = chosen.agent === "claude" ? "claude" : "codex";
    }
    this.opened = pop;
    this.narrowed = "";
    this.pickerReach = PICKER_ROWS;
    this.popSteady ??= new Steady(this.pop, true);
    this.popSteady.free();
    this.draw();
    if (!opening) return;
    // It opens with the one chosen in view, and keeps that size.
    this.pop.querySelector(".agent-row.chosen")?.scrollIntoView({ block: "nearest" });
    this.popSteady.hold();
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-pop], [data-choice]");
    if (!target || target.disabled || this.busy) return;
    if (target.dataset.pop) {
      const pop = target.dataset.pop as Pop;
      this.choose(this.opened === pop ? null : pop);
      return;
    }
    const value = target.dataset.value ?? "";
    switch (target.dataset.choice) {
      case "pick":
        this.pick = value;
        this.model = null;
        this.effort = null;
        this.chosenHere = false;
        this.opened = null;
        break;
      case "account":
        this.account = value;
        break;
      case "kind": {
        // The panel takes the other agent's table, growing or shrinking
        // upward, the choice under the pointer where it was.
        const kind: PickerKind = value === "claude" ? "claude" : "codex";
        if (kind !== this.kind) {
          this.kind = kind;
          this.pickerReach = PICKER_ROWS;
          remember("agent", kind);
          resizeUpward(this.pop, this.popSteady, () => this.draw());
        }
        return;
      }
      case "fold": {
        // A machine or a person opens or closes where it is, what is under
        // it sliding in or out within the panel's size.
        const open = target.getAttribute("aria-expanded") !== "true";
        this.pickerFolds.set(value, open);
        remember("pickerFolds", JSON.stringify([...this.pickerFolds]));
        // A person opened is drawn whole, the rows after it as they were.
        if (open) this.pickerReach += this.pickerInside.get(value) ?? 0;
        const html = this.agentsHtml(this.lines(this.options.store.world.peek()));
        // A person folded has nothing drawn under it: what opens is put in
        // first, then slides in.
        if (open) fillFold(this.pop, value, html);
        foldInPicker(this.pop, value, open);
        this.agentsDrawn = html;
        return;
      }
      case "town":
        this.town = value || null;
        // Another project: its machines and places, whatever was chosen.
        this.focusTouched = false;
        this.opened = null;
        break;
    }
    this.draw();
    if (!this.opened && !coarse()) this.input.focus({ preventScroll: true });
  }

  private draw(): void {
    const { words, store } = this.options;
    const world = store.world.peek();
    const lines = this.lines(world);
    // The login chosen gives way, when it cannot begin, to the one used
    // last on this computer, else the first that can.
    const all = lines.flatMap((line) => line.options.map((option) => ({ option, line })));
    const can = (key: string | null) => all.some(({ option, line }) => option.key === key && this.startable(option, line));
    if (!can(this.pick)) {
      const here = lines.find((line) => line.here);
      const agent = remembered("agent");
      const there = this.machine ? all.find(({ option }) => option.written === this.machine && can(option.key))?.option : undefined;
      const usual = there ?? here?.options.find((option) => option.agent === agent && can(option.key)) ?? here?.options.find((option) => can(option.key));
      // While machines are still answering, a choice made is kept.
      if (!this.askingAll || !all.some(({ option }) => option.key === this.pick)) {
        this.pick = usual?.key ?? all.find(({ option, line }) => this.startable(option, line))?.option.key ?? this.pick;
      }
    }
    const option = this.chosenOption(lines);
    const line = lines.find((one) => option && one.options.includes(option)) ?? null;
    const logins = this.openers(world, option);
    const login = logins.chosen;
    // The projects of the account it works for, wherever the agent lives: a
    // project's machines are where it works, not where its agents may be.
    const group = login ? groupsOf(world).find((one) => sameLogin(one.login, login)) : null;
    const towns: Choice[] = [
      { value: "", label: words.outsideProject },
      ...(world?.towns ?? [])
        .filter((town) => !!option && (!group || group.towns.includes(town.town)))
        .map((town) => ({ value: town.town, label: town.record.name })),
    ];
    // A project the agent cannot work in is kept while the machines are
    // still answering, and given up once they have.
    if (this.town && !towns.some((choice) => choice.value === this.town) && !this.askingAll && option) this.town = null;
    this.settleFocus(world);
    this.drawStage(option, world);
    this.drawContext(option, line, towns, world);
    this.drawPop(lines, logins, towns);
    this.drawModel();
    // What was sent went for another machine, agent or project: it is sent
    // again, to the one chosen now, when the conversation begins.
    if (this.draft && this.draft.for !== this.chosen()) {
      this.draft = null;
      this.tray?.reset();
    }
    this.input.placeholder = words.say(option ? programName(option.agent) : null);
    this.input.setAttribute("aria-label", this.input.placeholder);
    this.placeFiles();
    this.drawStart();
  }

  /** Above the input: the chosen agent's mark, drawn by hand, and the name
   *  of the project the words go to. The mark is drawn again only for
   *  another agent. */
  private drawStage(option: Option | null, world: WorldNow | null): void {
    const { words } = this.options;
    const town = this.town ? world?.towns.find((one) => one.town === this.town) : null;
    const title = this.element.querySelector<HTMLElement>(".new-talk-title")!;
    const name = town?.record.name ?? words.title;
    if (title.textContent !== name) title.textContent = name;
    const agent = option ? markOf(option.agent) : "";
    if (agent === this.marked) return;
    this.marked = agent;
    const mark = this.element.querySelector<HTMLElement>(".new-talk-mark")!;
    mark.innerHTML = agent ? glyph(agent, 48) : "";
    for (const path of mark.querySelectorAll("path, rect, circle")) path.setAttribute("pathLength", "1");
  }

  /** The labels over the input: the agent, as its program and the machine
   *  it lives on; the project; the machines and folders it is pointed at.
   *  Each opens its choices. */
  private drawContext(option: Option | null, line: Line | null, towns: Choice[], world: WorldNow | null): void {
    const { words, focusWords } = this.options;
    const label = (pop: Pop, mark: Parameters<typeof glyph>[0], body: string, title: string, extra = "") =>
      `<button type="button" class="talk-chip${extra}" data-pop="${pop}" aria-haspopup="dialog" aria-expanded="${this.opened === pop}" title="${escape(title)}">${glyph(mark, 16)}${body}${glyph("chevron", 12)}</button>`;
    const chips: string[] = [];
    if (option && line) {
      const who = this.who(option, line);
      const asking = !this.factsOf(option) && this.askingAll;
      const assistant = option.assistant;
      const sub = assistant ? [words.assistFrom(assistant.owner ?? ""), assistant.until ? words.assistUntil(new Date(assistant.until), new Date()) : ""].filter(Boolean).join(" ") : option.written;
      const said = [programName(option.agent), sub, who].filter(Boolean).join(" ");
      chips.push(
        label(
          "agent",
          markOf(option.agent),
          `<span class="talk-chip-name">${escape(sub)}</span>${who ? `<span class="talk-chip-sub who">${escape(who)}</span>` : ""}${asking ? `<span class="talk-chip-sub">${escape(words.asking)}</span>` : ""}`,
          option.assistant ? `${said}
${words.assistantWorks(option.assistant.owner ?? "")}` : said,
          option.assistant ? " assist" : "",
        ),
      );
    } else {
      chips.push(label("agent", "agents", `<span class="talk-chip-name">${escape(this.askingAll ? words.asking : words.agent)}</span>`, words.agent, " empty"));
    }
    if (towns.length > 1 || this.town) {
      const name = this.town ? (world?.towns.find((one) => one.town === this.town)?.record.name ?? this.town) : words.outsideProject;
      chips.push(label("town", "folder", `<span class="talk-chip-name">${escape(name)}</span>`, `${words.project}\n${name}`, this.town ? "" : " quiet"));
    }
    if (option) {
      const summary = focusSummary(this.focus, focusWords);
      chips.push(
        summary
          ? label("focus", "machine", summary.html, `${focusWords.title}\n${summary.said}`)
          : label("focus", "machine", `<span class="talk-chip-name">${escape(focusWords.none)}</span>`, focusWords.title, " quiet"),
      );
    }
    const html = chips.join("");
    if (this.context.innerHTML !== html) this.context.innerHTML = html;
  }

  /** The choices of the label open, risen over the input; on a phone, a
   *  sheet from the bottom. */
  private drawPop(lines: Line[], logins: { all: Login[]; chosen: Login | null }, towns: Choice[]): void {
    const { words } = this.options;
    const pop = this.pop;
    pop.hidden = !this.opened;
    pop.dataset.pop = this.opened ?? "";
    // The machines and folders draw themselves, and keep where the user
    // has walked to while the rest of the page is drawn again.
    if (this.opened === "focus") {
      if (pop.firstElementChild !== this.focusPanel.element || pop.childElementCount !== 1) pop.replaceChildren(this.focusPanel.element);
      this.focusPanel.refresh();
      return;
    }
    let html = "";
    if (this.opened === "agent") {
      this.drawAgents(agentKinds(this.pickerWords(), this.kind), this.agentsHtml(lines, logins));
      if (!this.keepSize) this.popSteady?.settle();
      this.keepSize = false;
      return;
    } else if (this.opened === "town") {
      html = `<h3 class="picker-title">${escape(words.project)}</h3>${this.list("town", words.project, towns, this.town ?? "")}`;
    }
    if (pop.innerHTML !== html) {
      const scroll = pop.scrollTop;
      pop.innerHTML = html;
      pop.scrollTop = scroll;
    }
    this.popSteady?.settle();
  }

  /** The table of agents and, where this device holds several accounts,
   *  which of them it works for. */
  private agentsHtml(lines: Line[], logins = this.openers(this.options.store.world.peek(), this.chosenOption(lines))): string {
    const { words } = this.options;
    const account =
      logins.all.length > 1
        ? `<div class="pop-section"><h3 class="picker-title">${escape(words.account)}</h3>${this.pills(
            "account",
            words.account,
            logins.all.map((one) => ({ value: `${one.hub}/${one.account}`, label: loginLabel(one) })),
            logins.chosen ? `${logins.chosen.hub}/${logins.chosen.account}` : "",
          )}</div>`
        : "";
    return `${this.picker(lines).html}${account}`;
  }

  /** The table of agents, and at the panel's foot, nearest the input, the
   *  field that narrows it and the choice of Codex or Claude: as the table
   *  changes, the panel grows or shrinks upward and they stay where they
   *  are. The field and what is typed in it stay as the table is drawn
   *  again. */
  private drawAgents(kinds: string, html: string): void {
    const pop = this.pop;
    let foot = pop.querySelector<HTMLElement>(":scope > .picker-foot");
    let list = pop.querySelector<HTMLElement>(":scope > .agent-list");
    if (!list || !foot) {
      pop.replaceChildren();
      list = document.createElement("div");
      list.className = "agent-list";
      foot = document.createElement("div");
      foot.className = "picker-foot";
      const search = document.createElement("label");
      search.className = "picker-search";
      search.innerHTML = glyph("search", 16);
      const field = document.createElement("input");
      field.className = "agent-filter";
      field.type = "search";
      field.placeholder = this.options.words.narrow;
      field.setAttribute("aria-label", this.options.words.narrow);
      field.autocomplete = "off";
      field.spellcheck = false;
      field.addEventListener("input", () => {
        this.narrowed = field.value;
        this.pickerReach = PICKER_ROWS;
        resizeUpward(pop, this.popSteady, () => this.draw());
      });
      search.append(field);
      foot.append(search);
      pop.append(list, foot);
      this.agentsDrawn = "";
    }
    const shown = foot.querySelector<HTMLElement>(":scope > .picker-kinds");
    if ((shown?.outerHTML ?? "") !== kinds) {
      shown?.remove();
      foot.insertAdjacentHTML("beforeend", kinds);
    }
    if (this.agentsDrawn !== html) {
      const scroll = pop.scrollTop;
      list.innerHTML = html;
      this.agentsDrawn = html;
      pop.scrollTop = scroll;
      this.pickerUnwatch();
      this.pickerUnwatch = whenNear(list, pop, () => {
        this.pickerReach += PICKER_ROWS;
        this.keepSize = true;
        this.draw();
      });
    }
  }

  /** The machines its account may use: where it lives and those the
   *  account may work on; for an assisting agent, the account's own. */
  private focusPlaces(): FocusPlace[] {
    const world = this.options.store.world.peek();
    const option = this.chosenOption();
    const login = this.loginFor(world, option);
    const lives = this.lives(world, option);
    const machines = option?.assistant ? (login ? filePlaces(world, login) : []) : lives ? workPlaces(world, lives, login) : [];
    return machines.map((machine) => focusPlace(world, machine));
  }

  /** The places the project chosen names on a machine. */
  private projectMarks(machine: string): { name: string; path: string }[] {
    const record = this.town ? this.options.store.world.peek()?.towns.find((one) => one.town === this.town)?.record : null;
    const name = machine.split(".")[0];
    return (record?.places ?? []).filter((place) => place.machine === name || place.machine === machine).map((place) => ({ name: place.what, path: place.path }));
  }

  /** The machines and folders, kept to those its account may use; in a
   *  project, until the user changes them, its machines and the places
   *  its record lists on them. */
  private settleFocus(world: WorldNow | null): void {
    const places = this.focusPlaces();
    if (!places.length) return;
    let focus = this.focus;
    if (!this.focusTouched && this.focusFrom?.town !== this.town) {
      focus = this.projectFocus(world, places);
      this.focusFrom = { town: this.town };
    } else {
      focus = { machines: focus.machines.filter((one) => places.some((place) => place.written === one.machine)) };
    }
    if (JSON.stringify(focus) === JSON.stringify(this.focus)) return;
    this.focus = focus;
    if (this.opened !== "focus") this.focusPanel.set(focus);
  }

  private projectFocus(world: WorldNow | null, places: FocusPlace[]): Focus {
    const record = this.town ? world?.towns.find((one) => one.town === this.town)?.record : null;
    if (!record) return noFocus();
    return {
      machines: record.machines.flatMap((name) => {
        const place = places.find((one) => one.name === name);
        const folders = [...new Set(record.places.filter((listed) => listed.machine === name).map((listed) => listed.path))];
        return place ? [{ machine: place.written, folders }] : [];
      }),
    };
  }

  /** The model and effort, as the conversation's own chip and panel show
   *  them: the chosen login's models, the one chosen here, else the one
   *  the account's preferences name, else the one chosen last on this
   *  device for this agent, else the program's default. */
  private drawModel(): void {
    const option = this.chosenOption();
    const models = this.modelsOf(option);
    const agent = option?.agent ?? "";
    const account = option ? (this.options.defaults?.of(this.loginFor(this.options.store.world.peek(), option), agent) ?? null) : null;
    const here = this.chosenHere ? this.model : null;
    const model =
      models.find((m) => m.model === here) ??
      models.find((m) => m.model === account?.model) ??
      models.find((m) => m.model === keptModel(agent)) ??
      models.find((m) => m.is_default) ??
      models[0] ??
      null;
    this.model = model?.model ?? null;
    const efforts = model?.reasoning_efforts.map((e) => e.effort) ?? [];
    this.effort =
      [this.chosenHere ? this.effort : null, account?.effort, keptEffort(agent, this.model), model?.default_reasoning_effort].find((e) => e && efforts.includes(e)) ?? null;
    const offers: Offers = {
      controls: models.length ? ["model"] : [],
      commands: [],
      models: models.map((m) => ({
        id: m.model,
        name: m.display_name,
        description: m.description || null,
        efforts: m.reasoning_efforts.map((e) => e.effort),
        default_effort: m.default_reasoning_effort || null,
        default: m.is_default,
        service_tiers: [],
        default_service_tier: null,
      })),
      model: this.model,
      effort: this.effort,
      service_tier: null,
      usage: null,
    };
    this.models.update(offers);
  }

  /** A computer keeps its originals; a phone uses the conversation's machine,
   *  but for an agent assisting the user, which cannot reach that computer,
   *  one of the user's own. */
  private placeFiles(): void {
    const world = this.options.store.world.peek();
    const option = this.chosenOption();
    const here = machinesOf(world).find((machine) => machine.here)?.written ?? null;
    this.filesOn = option ? (option.assistant ? fileHome(world, option.assistant.login) : (here ?? option.written)) : null;
    // No machine of the user's can take files: only text and images go,
    // said the moment one is offered (`world-run.md`, 受协助的人附的文件).
    const pick = this.element.querySelector<HTMLInputElement>(".composer-pick");
    if (pick) pick.accept = this.imagesOnly() ? "image/*" : "";
  }

  /** Why only images can be attached now: the agent assists the user, and
   *  no machine of theirs can take a file; null when any file can. */
  private imagesOnly(): string | null {
    return this.chosenOption()?.assistant && !this.filesOn && this.options.attach ? this.options.attach.words.imagesOnly : null;
  }

  /** A file offered that cannot be taken, said under the words. */
  private refuseFile(why: string): void {
    this.note.textContent = why;
    this.note.classList.add("refused");
  }

  private drawStart(): void {
    const lines = this.lines(this.options.store.world.peek());
    const option = this.chosenOption(lines);
    const line = lines.find((one) => option && one.options.includes(option));
    const said = this.input.value.trim() !== "" || (this.tray?.count ?? 0) > 0;
    const filesNowhere = !!option?.assistant && (this.tray?.files ?? 0) > 0 && !this.filesOn;
    this.start.disabled = this.busy || !option || !line || !this.startable(option, line) || !said || filesNowhere;
    // No machine of the user's can take the files: said, as nothing else
    // would say why it cannot begin.
    const nowhere = this.options.words.noFilePlace;
    if (filesNowhere && !this.busy) {
      this.note.textContent = nowhere;
      this.note.classList.add("refused");
    } else if (this.note.textContent === nowhere) {
      this.note.textContent = "";
      this.note.classList.remove("refused");
    }
  }

  /** Claude starts on a machine only once the user has let it work
   *  without asking there: asked now, when it is needed. Not knowing is
   *  not a no; Claude Code then says for itself. */
  private async allowed(option: Option): Promise<boolean> {
    const facts = this.factsOf(option);
    if (facts?.unattended?.kind !== "known" || facts.unattended.value) return true;
    const several = this.logins(option.machine, "claude").length > 1;
    const home: ConsentHome = { dir: homeChoice(facts), label: several ? homeLabel(facts, this.options.homeWords) : null };
    if (!this.options.allowClaude || !(await this.options.allowClaude(option.machine, this.options.words.allowAndStart, home))) return false;
    facts.unattended = { kind: "known", value: true };
    this.draw();
    return true;
  }

  private async submit(): Promise<void> {
    const text = this.input.value.trim();
    const option = this.chosenOption();
    if (this.busy || !option || this.start.disabled || (!text && !this.tray?.count)) return;
    if (option.agent === "claude" && !(await this.allowed(option))) return;
    this.busy = true;
    this.choose(null);
    this.drawStart();
    this.note.classList.remove("refused");
    this.note.title = "";
    remember("pick", option.key);
    remember("agent", option.agent);
    const world = this.options.store.world.peek();
    const login = this.loginFor(world, option);
    if (login && !option.assistant) remember("account", `${login.hub}/${login.account}`);
    keepChoice(option.agent, this.model, this.effort);
    const { words } = this.options;
    // The words go into the conversation at once; what happens on the way
    // stays out of sight unless it fails.
    if (this.starting) {
      this.starting.begin({
        text,
        attached: (this.tray?.held ?? []).map(({ name, size, image, preview }) => ({ name, size, image, preview })),
      });
      this.element.classList.add("begun");
      this.input.value = "";
      this.fit();
    }
    try {
      if (this.tray?.count) {
        if (!this.starting) this.note.textContent = words.sendingFiles;
        try {
          await this.tray.sendAll();
        } catch (error) {
          throw new NotSent(String(error));
        }
      }
      if (!this.starting) this.note.textContent = words.starting;
      const address = await this.options.begin({
        agent: option.agent,
        machine: option.written,
        town: this.town,
        text,
        model: this.model,
        effort: this.effort,
        attachments: (this.tray?.arrived ?? []) as Attachment[],
        account: login ? { hub: login.hub, account: login.account } : null,
        focus: this.focus,
        home: option.home,
        filesOn: option.assistant && (this.tray?.files ?? 0) > 0 ? this.filesOn : null,
        mcpOff: this.mcpOff,
      });
      // Begun: the page is empty again for the next conversation here.
      this.input.value = "";
      remember(this.kept, null);
      this.fit();
      this.tray?.take();
      this.draft = null;
      this.town = this.options.town;
      this.focusTouched = false;
      this.focusFrom = null;
      this.focus = noFocus();
      this.mcpOff = [];
      this.focusPanel.set(this.focus);
      this.busy = false;
      this.note.textContent = "";
      this.options.opened(address, this.starting?.seed() ?? null);
    } catch (error) {
      this.busy = false;
      const why = error instanceof Error ? error.message : String(error);
      if (this.starting) {
        // Said under the words that went; the words come back to the input
        // to be sent again.
        this.starting.failed(error instanceof NotSent ? `${words.filesNotSent}: ${why}` : why);
        if (!this.input.value) {
          this.input.value = text;
          this.fit();
        }
      } else {
        this.note.textContent = error instanceof NotSent ? words.filesNotSent : words.notOpened(why);
        this.note.title = error instanceof NotSent ? "" : why;
        this.note.classList.add("refused");
      }
    } finally {
      this.drawStart();
    }
  }
}

/** A file attached to the first words did not reach the machine; the tray
 *  says which, and why. */
class NotSent extends Error {}

/** Puts what is under the fold `key` in `html` under that fold as drawn in
 *  `root`, where nothing was drawn while it was folded. */
function fillFold(root: HTMLElement, key: string, html: string): void {
  const inner = [...root.querySelectorAll<HTMLElement>(".agent-fold")].find((one) => one.dataset.fold === key)?.querySelector<HTMLElement>(":scope > .agent-fold-inner");
  if (!inner || inner.childElementCount) return;
  const drawn = document.createElement("template");
  drawn.innerHTML = html;
  const fresh = [...drawn.content.querySelectorAll<HTMLElement>(".agent-fold")].find((one) => one.dataset.fold === key)?.querySelector<HTMLElement>(":scope > .agent-fold-inner");
  if (fresh) inner.replaceChildren(...fresh.childNodes);
}
