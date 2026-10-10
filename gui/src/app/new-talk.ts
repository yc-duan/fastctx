// Beginning a conversation (`world-gui.md`, 界面的结构): which agent, as
// one of its logins, chosen among those the user holds, grouped as they hold
// them: on their own machines, on machines of others they may work on
// (shared with them, or a member's to a super administrator), and lent to
// them by another account. Each says which program, on which machine, who
// is logged in and how much of its usage is spent, as its own machine says.
// Then the account it works for where more than one may open it there,
// which model and how hard it thinks, which project, the machine it works
// on (the one it lives on unless another is chosen, `world-run.md`, 这里),
// whether it may ask for colleagues, where they would live and as which
// Codex login there, and the first words. An agent lent to the user works
// on no machine of its own choosing: each of its commands names one of the
// user's machines, so it is asked no machine to work on, and the files
// attached for it, other than images, go to a machine of the user's chosen
// as they are attached. The first time the user is about to use an agent
// on a machine of another, what that means is said before it begins: that
// machine may then reach the user's machines (`world-network.md`).
//
// A sheet over the page, opened by the button in the sidebar or Ctrl+N; on
// a phone it rises from the foot of the screen. Every machine is asked what
// agents it has once each time the sheet opens. Once the agent is asked in,
// its conversation opens and its first turn is watched from the start.
//
// Files may come with the first words. They wait in the sheet while the
// agent and project are still being chosen, and go when the conversation is
// begun: the first makes a place on the agent's machine for the agent about
// to be asked in, the rest join it there, and the agent starts in that
// place with all of them.

import { AttachTray, type AttachSource, type AttachWords } from "../conversation/attach";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import type { HubAccount, Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentFacts, AgentModel, WorldAgents } from "../world/agents";
import { accountOf, homeChoice, homeLabel, homesOf, installed, loggedOut, windowsShort, type HomeWords } from "./agent-homes";
import type { ConsentHome } from "./claude-consent";
import type { Attachment, UploadTarget } from "../world/attachments";
import type { WorldNow } from "../world/bridge";
import type { Address } from "../world/events";
import {
  borrowedOf,
  colleagueHomes,
  filePlaces,
  groupsOf,
  isSuper,
  loginKey,
  loginLabel,
  machineActsFor,
  machinesOf,
  openersOn,
  ownerOf,
  owns,
  sameLogin,
  workPlaces,
  type Borrowed,
  type Machine,
} from "./reach";
import type { Store } from "./store";
import type { NewTalkWords } from "./words";

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
   *  agent is lent to one. */
  account: HubAccount | null;
  /** The machine it works on, as written, when not the one it lives on. */
  worksOn: string | null;
  /** Where its colleagues live; without it, it asks for none. */
  colleagues: string | null;
  /** The agent's login on the machine it lives on, as its home's
   *  directory; null for the default one. */
  home: string | null;
  /** The Codex login its colleagues use where they live; null for the
   *  default one there. */
  colleaguesHome: string | null;
  /** Where its files other than images go, for an agent lent to the user. */
  filesOn: string | null;
}

export interface NewTalkOptions {
  store: Store;
  words: NewTalkWords;
  homeWords: HomeWords;
  /** What the machines' own programs say of the agents on them: one, or
   *  with null every machine reached. */
  agents(machine: string | null): Promise<WorldAgents>;
  begin(begun: Begun): Promise<Address>;
  opened(address: Address): void;
  /** Asks the user to let Claude work without asking in one of its logins
   *  on a machine, where it has not yet; true once Claude Code there
   *  records it. */
  allowClaude?(machine: string, go: string, home: ConsentHome): Promise<boolean>;
  /** Attaching files to the first words, where the World can carry them. */
  attach?: {
    words: AttachWords;
    /** Carries one file to a machine for the conversation about to begin. */
    upload(machine: string, target: UploadTarget, source: AttachSource, progress: (sent: number) => void): Promise<Attachment>;
    /** Files dropped onto the window, by path, where the shell gives them
     *  so; returns how to stop listening. */
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
 *  or another's with only an agent's login lent to them. */
type Hold = "mine" | "others" | "lent";

/** One agent's login a conversation may begin with. */
interface Option {
  key: string;
  machine: string;
  written: string;
  agent: string;
  /** As the World is told it: the home's directory, or null for the
   *  default one. */
  home: string | null;
  /** Lent to one of the user's logins. */
  lent: Borrowed | null;
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

const agentsKnown = ["codex", "claude"];
const programName = (agent: string) => (agent === "codex" ? "Codex" : agent === "claude" ? "Claude" : agent);

/** What is known of the agents on one machine. */
type Asked = { state: "asking" } | { state: "known"; facts: AgentFacts[] } | { state: "unknown"; why: string } | { state: "offline" };

function remembered(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function remember(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not kept: the next sheet starts from the usual choice.
  }
}

/** A login an agent can start in: its program is there and it is not
 *  known to be logged out. */
function usable(facts: AgentFacts): boolean {
  return installed(facts) && !loggedOut(facts);
}


export class NewTalk {
  private readonly dialog: HTMLDialogElement;
  private readonly form: HTMLFormElement;
  private readonly input: HTMLTextAreaElement;
  private readonly note: HTMLElement;
  private readonly start: HTMLButtonElement;
  /** The login chosen, by its key; the one used last where none is. */
  private pick: string | null = remembered("pick");
  private town: string | null = null;
  private model: string | null = null;
  private effort: string | null = null;
  /** The account chosen, as `hub/account`. */
  private account: string | null = remembered("account");
  /** Where it works and where its colleagues live, as written; null for
   *  the machine it lives on, and for no colleagues. */
  private worksOn: string | null = null;
  private colleagues: string | null = null;
  /** The Codex login its colleagues use where they live, as its home's
   *  directory or null for the default one; undefined until one is, when
   *  the one used last on that machine is taken. */
  private colleaguesHome: string | null | undefined = undefined;
  /** Where the files of a lent agent go, as written. */
  private filesOn: string | null = null;
  private asked = new Map<string, Asked>();
  /** Every machine is being asked about its agents. */
  private askingAll = false;
  /** The logins are all shown to choose among; otherwise only the one
   *  chosen, with a way to choose another. */
  private choosing = false;
  private busy = false;
  private tray: AttachTray | null = null;
  /** The place on a machine the files sent so far went to, and for which
   *  machine, agent and project; another choice needs another place. */
  private draft: { resident: string; for: string } | null = null;
  private stopDrops: (() => void) | null = null;

  constructor(private readonly options: NewTalkOptions) {
    const { words } = options;
    this.dialog = document.createElement("dialog");
    this.dialog.className = "sheet new-talk-sheet";
    this.dialog.setAttribute("aria-label", words.title);
    this.dialog.innerHTML = `
      <form class="sheet-body" method="dialog">
        <header class="sheet-head">
          <h2 class="sheet-title">${escape(words.title)}</h2>
          <button class="icon-button sheet-close" type="button" aria-label="${escape(words.cancel)}">${glyph("close", 18)}</button>
        </header>
        <div class="choices"></div>
        <div class="first-words-box">
          <div class="sheet-tray"></div>
          <div class="files-on"></div>
          <textarea class="first-words" rows="3" placeholder="${escape(words.firstWords)}" aria-label="${escape(words.firstWords)}"></textarea>
        </div>
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="sheet-foot">
          <span class="sheet-tools"><button class="icon-button composer-attach" type="button" hidden></button><input class="composer-pick" type="file" multiple hidden><button class="icon-button composer-photo" type="button" hidden></button><input class="composer-shot" type="file" accept="image/*" capture="environment" hidden><span class="sheet-hint">${escape(words.hint)}</span></span>
          <button class="sheet-go" type="submit">${escape(words.start)}</button>
        </footer>
      </form>`;
    document.body.append(this.dialog);
    this.form = this.dialog.querySelector("form")!;
    this.input = this.dialog.querySelector("textarea")!;
    this.note = this.dialog.querySelector<HTMLElement>(".sheet-note")!;
    this.start = this.dialog.querySelector<HTMLButtonElement>(".sheet-go")!;
    this.dialog.querySelector(".sheet-close")!.addEventListener("click", () => this.close());
    // A click on the backdrop, outside the sheet, closes it.
    this.dialog.addEventListener("click", (event) => {
      if (event.target === this.dialog) this.close();
    });
    this.dialog.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-choice]");
      if (!button || button.disabled || this.busy) return;
      const value = button.dataset.value!;
      switch (button.dataset.choice) {
        case "expand":
          this.choosing = !this.choosing;
          break;
        case "pick":
          this.pick = value;
          this.choosing = false;
          this.worksOn = null;
          this.colleagues = null;
          this.colleaguesHome = undefined;
          this.model = null;
          this.effort = null;
          break;
        case "account":
          this.account = value;
          this.worksOn = null;
          this.colleagues = null;
          break;
        case "works":
          this.worksOn = value || null;
          break;
        case "colleagues":
          this.colleagues = value || null;
          this.colleaguesHome = undefined;
          if (button.dataset.machine) this.ask(button.dataset.machine);
          break;
        case "colleagues-home":
          this.colleaguesHome = value || null;
          break;
        case "model":
          this.model = value;
          this.effort = null;
          break;
        case "effort":
          this.effort = value;
          break;
        case "files-on":
          this.filesOn = value;
          remember("files-on", value);
          break;
        default:
          this.town = value || null;
      }
      redrawKeepingFocus(this.dialog, () => this.draw());
    });
    this.form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    this.input.addEventListener("input", () => this.drawStart());
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !matchMedia("(pointer: coarse)").matches) {
        event.preventDefault();
        void this.submit();
      }
    });
    this.dialog.addEventListener("close", () => {
      this.stopDrops?.();
      this.stopDrops = null;
    });
    if (options.attach) this.attaching(options.attach);
  }

  /** Files come with the first words: picked with the paperclip, pasted,
   *  or dropped onto the sheet. */
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
          const done = await attach.upload(option.machine, target, source, progress);
          this.draft = { resident: done.owner.resident, for: chosen };
          return done;
        },
      },
      () => {
        // A file other than an image asks, for a lent agent, where it goes.
        this.drawFilesOn();
        this.drawStart();
      },
      true,
    );
    this.tray = tray;
    this.dialog.querySelector(".sheet-tray")!.replaceWith(tray.element);
    const button = this.dialog.querySelector<HTMLButtonElement>(".composer-attach")!;
    const pick = this.dialog.querySelector<HTMLInputElement>(".composer-pick")!;
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
    if (matchMedia("(pointer: coarse)").matches) {
      const photo = this.dialog.querySelector<HTMLButtonElement>(".composer-photo")!;
      const shot = this.dialog.querySelector<HTMLInputElement>(".composer-shot")!;
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
    this.form.append(veil);
    // In a browser, files dragged over the sheet; in the shell, the window
    // takes the drop and gives their paths, heard while the sheet is open.
    let depth = 0;
    this.form.addEventListener("dragenter", (event) => {
      if (!event.dataTransfer?.types.includes("Files")) return;
      depth += 1;
      veil.hidden = false;
    });
    this.form.addEventListener("dragleave", () => {
      depth = Math.max(0, depth - 1);
      if (!depth) veil.hidden = true;
    });
    this.form.addEventListener("dragover", (event) => {
      if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
    });
    this.form.addEventListener("drop", (event) => {
      depth = 0;
      veil.hidden = true;
      if (!event.dataTransfer?.files.length) return;
      event.preventDefault();
      if (!this.busy) tray.addFiles(event.dataTransfer.files);
    });
    this.listen = () =>
      attach.drops?.({
        over: (over) => {
          if (this.dialog.open) veil.hidden = !over;
        },
        dropped: (paths) => {
          veil.hidden = true;
          if (this.dialog.open && !this.busy) tray.addPaths(paths);
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

  /** Opens the sheet; `town` is the project to begin in, when opened from
   *  one, and `pick` the login to begin with, when one was chosen. */
  open(town: string | null = null, pick: string | null = null): void {
    if (pick) this.pick = pick;
    this.choosing = false;
    this.town = town;
    this.model = null;
    this.effort = null;
    this.worksOn = null;
    this.colleagues = null;
    this.colleaguesHome = undefined;
    this.note.hidden = true;
    // What is on each machine is asked again each time: a program may have
    // been installed, logged in or updated since.
    this.asked.clear();
    this.askAll();
    this.draw();
    if (!this.dialog.open) {
      this.dialog.showModal();
      this.stopDrops = this.listen();
    }
    this.input.focus();
  }

  close(): void {
    if (this.busy) return;
    this.dialog.close();
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
        if (this.dialog.open) this.draw();
      });
  }

  /** Asks one machine what agents it has, where the asking of all did
   *  not answer for it. */
  private ask(machine: string): void {
    if (this.asked.has(machine) || this.askingAll) return;
    this.asked.set(machine, { state: "asking" });
    this.options
      .agents(machine)
      .then((world) => {
        const answer = world.machines.find((m) => (m.kind === "answered" ? m.reply.machine : m.machine) === machine);
        this.asked.set(
          machine,
          answer?.kind === "answered"
            ? { state: "known", facts: answer.reply.agents }
            : answer?.kind === "offline"
              ? { state: "offline" }
              : { state: "unknown", why: answer?.kind === "no_answer" ? answer.reason : "" },
        );
      })
      .catch((error: unknown) => this.asked.set(machine, { state: "unknown", why: String(error) }))
      .finally(() => {
        if (this.dialog.open) this.draw();
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
              .filter(installed)
              .map((home) => this.option(machine.name, machine.written, agent, homeChoice(home), null)),
          )
        : agentsKnown.map((agent) => this.option(machine.name, machine.written, agent, null, null));
      lines.push({ hold: mine ? "mine" : "others", machine: machine.name, written: machine.written, here: machine.here, online: machine.here ? true : machine.online, owner, shared, options });
    }
    for (const lent of borrowedOf(world)) {
      let line = lines.find((one) => one.hold === "lent" && one.written === lent.written);
      if (!line) {
        line = { hold: "lent", machine: lent.machine, written: lent.written, here: false, online: lent.online, owner: lent.owner, shared: false, options: [] };
        lines.push(line);
      }
      if (!line.options.some((option) => option.agent === lent.agent && option.home === lent.home)) {
        line.options.push(this.option(lent.machine, lent.written, lent.agent, lent.home, lent));
      }
    }
    return lines;
  }

  private option(machine: string, written: string, agent: string, home: string | null, lent: Borrowed | null): Option {
    return { key: loginKey(machine, agent, home), machine, written, agent, home, lent };
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
   *  the one it works for: the only one, the one picked, or for a lent
   *  agent the login it is lent to. */
  private openers(world: WorldNow | null, option: Option | null): { all: Login[]; chosen: Login | null } {
    if (!option) return { all: [], chosen: null };
    if (option.lent) return { all: [option.lent.login], chosen: option.lent.login };
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
    if (!option || option.lent) return null;
    return machinesOf(world).find((machine) => machine.name === option.machine) ?? null;
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
  private models(option: Option | null): AgentModel[] {
    const facts = this.factsOf(option);
    return facts?.models.kind === "known" ? facts.models.value : [];
  }

  /** How much of its usage a login has spent, window by window, as
   *  Claude Code's own status line writes it: "5h 42% · 7d 18%"; and that
   *  it goes on with credits once a window is spent. Nothing where none
   *  was read. */
  private spent(facts: AgentFacts | null): { html: string; said: string } | null {
    const { words } = this.options;
    if (facts?.limits?.kind !== "known") return null;
    const windows = windowsShort(facts.limits);
    if (!windows.length) return null;
    const credits = windows.some((window) => window.level === "spent") && !!facts.limits.value.credits;
    const said = [...windows.map((window) => `${window.label} ${window.percent}`), credits ? words.onCredits : ""].filter(Boolean).join(" · ");
    const html = `<span class="login-limits">${windows
      .map((window) => `<span class="limit-short${window.level ? ` ${window.level}` : ""}">${escape(`${window.label} ${window.percent}`)}</span>`)
      .join("")}${credits ? `<span class="limit-short credits">${escape(words.onCredits)}</span>` : ""}</span>`;
    return { html, said };
  }

  /** One login as a choice: the program; who is logged in where the
   *  machine has several of its logins, or where it is lent; how much of
   *  each window is spent; or why it cannot begin. Said whole to a screen
   *  reader. */
  private optionChoice(option: Option, line: Line): string {
    const { words, homeWords } = this.options;
    const asked = this.asked.get(option.machine);
    const facts = this.factsOf(option);
    const name = programName(option.agent);
    let note = "";
    let who: string | null = null;
    let spent: ReturnType<NewTalk["spent"]> = null;
    let consent: string | null = null;
    if (line.online === false) note = words.offline;
    else if (!asked || asked.state === "asking" || (this.askingAll && !facts && asked.state !== "known")) note = words.asking;
    else if (asked.state === "offline") note = words.offline;
    else if (asked.state === "unknown") note = words.unsure;
    else if (!facts) note = option.lent ? words.lentGone : words.notInstalled;
    else if (loggedOut(facts)) note = words.loggedOut;
    else {
      const several = line.options.filter((other) => other.agent === option.agent).length > 1;
      who = several || option.lent ? (accountOf(facts).email ?? homeLabel(facts, homeWords)) : null;
      spent = this.spent(facts);
      consent = facts.unattended?.kind === "known" && !facts.unattended.value ? words.needsConsent : null;
    }
    const said = [name, who, spent?.said, consent, note].filter(Boolean).join(" · ");
    const meter = spent ? spent.html : "";
    const startable = this.startable(option, line);
    const chosen = option.key === this.pick;
    return `<button type="button" class="choice login-choice${chosen ? " chosen" : ""}" role="radio" aria-checked="${chosen}" aria-label="${escape(said)}" title="${escape(said)}" data-choice="pick" data-value="${escape(option.key)}"${startable ? "" : " disabled"}><span class="login-agent">${escape(name)}</span>${who ? `<span class="login-who">${escape(who)}</span>` : ""}${meter}${consent ? `<span class="choice-note caution">${escape(consent)}</span>` : ""}${note ? `<span class="choice-note">${escape(note)}</span>` : ""}</button>`;
  }

  /** The login chosen, in a line: the program, its machine and whose it
   *  is, how much is spent; and the way to choose another. */
  private chosenLine(option: Option, line: Line): string {
    const { words } = this.options;
    const facts = this.factsOf(option);
    const spent = facts && !loggedOut(facts) ? this.spent(facts) : null;
    const notes = [line.here ? words.here : "", option.lent ? words.lentBy(option.lent.owner ?? "") : line.owner && line.hold !== "mine" ? words.ownedBy(line.owner) : ""].filter(Boolean);
    const lentWhy = option.lent ? words.lentWorks(option.lent.owner ?? "") : "";
    const who = facts && (line.options.filter((other) => other.agent === option.agent).length > 1 || option.lent) ? (accountOf(facts).email ?? homeLabel(facts, this.options.homeWords)) : null;
    return `<div class="choice-row agent-chosen">
      <span class="choice-label">${escape(words.agent)}</span>
      <div class="choice-options">
        <span class="chosen-login${option.lent ? " lent" : ""}"${lentWhy ? ` title="${escape(lentWhy)}"` : ""}><span class="login-agent">${escape(programName(option.agent))}</span><span class="chosen-machine"><span class="dot" aria-hidden="true"></span>${escape(option.written)}</span>${notes.map((note) => `<span class="picker-note">${escape(note)}</span>`).join("")}${who ? `<span class="login-who">${escape(who)}</span>` : ""}${spent ? spent.html : ""}</span>
        <button type="button" class="text-button quiet-button chosen-change" data-choice="expand" data-value="" aria-expanded="false">${escape(words.another)}</button>
      </div>
    </div>`;
  }

  /** The agents the user may begin with, grouped as they hold them, a line
   *  a machine. */
  private picker(lines: Line[]): string {
    const { words } = this.options;
    const groups: { hold: Hold; title: string; lines: Line[] }[] = [
      { hold: "mine", title: words.mine, lines: lines.filter((line) => line.hold === "mine") },
      { hold: "others", title: words.others(lines.every((line) => line.hold !== "others" || line.shared)), lines: lines.filter((line) => line.hold === "others") },
      { hold: "lent", title: words.lent, lines: lines.filter((line) => line.hold === "lent") },
    ];
    const machineLine = (line: Line) => {
      const notes = [line.here ? words.here : "", line.owner && line.hold !== "mine" ? words.ownedBy(line.owner) : "", line.online === false ? words.offline : ""].filter(Boolean);
      const state = line.online === false ? "offline" : "online";
      const options = line.online === false ? "" : line.options.map((option) => this.optionChoice(option, line)).join("");
      const none = line.online !== false && !line.options.length ? `<span class="choice-none quiet">${escape(words.noAgents)}</span>` : "";
      return `<div class="picker-line ${state}"><span class="picker-machine"><span class="dot" aria-hidden="true"></span><span class="picker-name">${escape(line.written)}</span>${notes.map((note) => `<span class="picker-note">${escape(note)}</span>`).join("")}</span><div class="choice-options">${options}${none}</div></div>`;
    };
    const body = groups
      .filter((group) => group.lines.length)
      .map((group) => `<section class="picker-group picker-${group.hold}"><h3 class="picker-title">${escape(group.title)}</h3>${group.lines.map(machineLine).join("")}</section>`)
      .join("");
    return `<div class="agent-picker" role="radiogroup" aria-label="${escape(words.agent)}">
      <span class="choice-label picker-label">${escape(words.agent)}</span>
      <div class="picker-groups">${body || `<span class="choice-none">${escape(words.noMachine)}</span>`}</div>
    </div>`;
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
      const usual = here?.options.find((option) => option.agent === agent && can(option.key)) ?? here?.options.find((option) => can(option.key));
      // While machines are still answering, a choice made is kept.
      if (!this.askingAll || !all.some(({ option }) => option.key === this.pick)) {
        this.pick = usual?.key ?? all.find(({ option, line }) => this.startable(option, line))?.option.key ?? this.pick;
      }
    }
    const option = this.chosenOption(lines);
    const lent = option?.lent ?? null;
    const lives = this.lives(world, option);
    const logins = this.openers(world, option);
    const login = logins.chosen;
    // The projects of the account it works for that span where it lives.
    const group = login ? groupsOf(world).find((group) => sameLogin(group.login, login)) : null;
    const towns: Choice[] = [
      { value: "", label: words.noProject },
      ...(world?.towns ?? [])
        .filter((town) => !!option && town.record.machines.includes(option.machine) && (!group || group.towns.includes(town.town)))
        .map((town) => ({ value: town.town, label: town.record.name })),
    ];
    if (this.town && !towns.some((choice) => choice.value === this.town)) this.town = null;
    const places: Choice[] = lives
      ? workPlaces(world, lives, login).map((machine) => ({
          value: machine.written === lives.written ? "" : machine.written,
          label: machine.written === lives.written ? words.sameMachine : machine.written,
          note: machine.written !== lives.written && machine.online === false ? words.offline : undefined,
        }))
      : [];
    if (this.worksOn && !places.some((choice) => choice.value === this.worksOn)) this.worksOn = null;
    const colleagueMachines = option ? colleagueHomes(world, lives, login) : [];
    const homes: (Choice & { machine?: string })[] = option
      ? [
          { value: "", label: words.noColleagues },
          ...colleagueMachines.map((machine) => ({
            value: machine.written,
            label: machine.written,
            machine: machine.name,
            note: machine.here ? words.here : machine.lentBy ? words.lentBy(machine.lentBy) : machine.online === false ? words.offline : undefined,
          })),
        ]
      : [];
    if (this.colleagues && !homes.some((choice) => choice.value === this.colleagues)) this.colleagues = null;
    // Where colleagues live, the Codex login they use there: the only one
    // there, or of several the one chosen.
    const colleaguesOn = colleagueMachines.find((machine) => machine.written === this.colleagues)?.name ?? null;
    if (colleaguesOn) this.ask(colleaguesOn);
    const colleagueLogins = colleaguesOn ? this.logins(colleaguesOn, "codex") : [];
    this.colleaguesHome = settle(colleagueLogins, this.colleaguesHome, `home:${colleaguesOn}:codex`);
    const models = this.models(option);
    const agent = option?.agent ?? "";
    const model =
      models.find((m) => m.model === this.model) ??
      models.find((m) => m.model === remembered(`model:${agent}`)) ??
      models.find((m) => m.is_default) ??
      models[0] ??
      null;
    this.model = model?.model ?? null;
    const efforts = model?.reasoning_efforts.map((e) => e.effort) ?? [];
    const effort = [this.effort, remembered(`effort:${agent}`), model?.default_reasoning_effort].find((e) => e && efforts.includes(e)) ?? null;
    this.effort = effort;
    const row = (kind: string, label: string, choices: (Choice & { machine?: string })[], chosen: string | null, hint = "") =>
      `<div class="choice-row" role="radiogroup" aria-label="${escape(label)}">
        <span class="choice-label"${hint ? ` title="${escape(hint)}"` : ""}>${escape(label)}</span>
        <div class="choice-options">${
          choices.length
            ? choices
                .map(
                  (choice) =>
                    `<button type="button" class="choice${choice.value === (chosen ?? "") ? " chosen" : ""}" role="radio" aria-checked="${choice.value === (chosen ?? "")}" data-choice="${kind}" data-value="${escape(choice.value)}"${choice.machine ? ` data-machine="${escape(choice.machine)}"` : ""}${choice.disabled ? " disabled" : ""}>${escape(choice.label)}${choice.note ? `<span class="choice-note">${escape(choice.note)}</span>` : ""}</button>`,
                )
                .join("")
            : `<span class="choice-none">${escape(words.noMachine)}</span>`
        }</div>
      </div>`;
    // Using an agent on another's machine lets that machine reach the
    // account's machines, from the first time on: said before it begins.
    const owner = option ? (lent ? lent.owner : lives && login && !owns(lives, login) ? ownerOf(lives, login) : null) : null;
    const reaching = !!option && !!login && !!owner && !machineActsFor(world, option.machine, login);
    // The one chosen, in a line, while the rest are not asked for; all of
    // them while it is being chosen, or while none can begin.
    const line = lines.find((one) => option && one.options.includes(option));
    const folded = !this.choosing && !!option && !!line && this.startable(option, line);
    this.dialog.querySelector(".choices")!.innerHTML = [
      folded ? this.chosenLine(option!, line!) : this.picker(lines),
      logins.all.length > 1
        ? row(
            "account",
            words.account,
            logins.all.map((one) => ({ value: `${one.hub}/${one.account}`, label: loginLabel(one) })),
            login ? `${login.hub}/${login.account}` : null,
          )
        : "",
      models.length
        ? row(
            "model",
            words.model,
            models.map((m) => ({ value: m.model, label: m.display_name, note: m.is_default ? words.defaultModel : undefined })),
            this.model,
          )
        : "",
      efforts.length > 1 ? row("effort", words.effort, efforts.map((e) => ({ value: e, label: e })), this.effort) : "",
      towns.length > 1 ? row("town", words.project, towns, this.town) : "",
      !lent && places.length > 1 ? row("works", words.worksOn, places, this.worksOn ?? "") : "",
      homes.length > 1 ? row("colleagues", words.colleagues, homes, this.colleagues ?? "", this.colleagues ? words.colleaguesWhat(this.colleagues) : "") : "",
      colleagueLogins.length > 1 ? row("colleagues-home", words.colleaguesLogin, colleagueLogins, this.colleaguesHome ?? "") : "",
      reaching && owner && login && option ? `<p class="choice-margin choice-warn">${escape(words.reaches(owner, option.written, isSuper(world, login)))}</p>` : "",
    ].join("");
    // What was sent went for another machine, agent or project: it is sent
    // again, to the one chosen now, when the conversation begins.
    if (this.draft && this.draft.for !== this.chosen()) {
      this.draft = null;
      this.tray?.reset();
    }
    this.drawFilesOn();
    this.drawStart();
  }

  /** For a lent agent given files other than images: which of the user's
   *  machines they go to. */
  private drawFilesOn(): void {
    const { words, store } = this.options;
    const box = this.dialog.querySelector<HTMLElement>(".files-on")!;
    const world = store.world.peek();
    const option = this.chosenOption();
    const login = this.loginFor(world, option);
    if (!option?.lent || !login || !(this.tray?.files ?? 0)) {
      box.innerHTML = "";
      return;
    }
    const places = filePlaces(world, login);
    const usual = [this.filesOn, remembered("files-on")].find((written) => written && places.some((machine) => machine.written === written));
    this.filesOn = usual ?? places.find((machine) => machine.here)?.written ?? places.find((machine) => machine.online !== false)?.written ?? null;
    const html = `<div class="choice-row files-row" role="radiogroup" aria-label="${escape(words.filesOn)}">
      <span class="choice-label" title="${escape(words.filesOnWhat)}">${escape(words.filesOn)}</span>
      <div class="choice-options">${
        places.length
          ? places
              .map((machine) => {
                const chosen = machine.written === this.filesOn;
                const note = machine.here ? words.here : machine.online === false ? words.offline : "";
                return `<button type="button" class="choice${chosen ? " chosen" : ""}" role="radio" aria-checked="${chosen}" data-choice="files-on" data-value="${escape(machine.written)}"${machine.online === false && !machine.here ? " disabled" : ""}>${escape(machine.written)}${note ? `<span class="choice-note">${escape(note)}</span>` : ""}</button>`;
              })
              .join("")
          : `<span class="choice-none">${escape(words.noFilePlace)}</span>`
      }</div>
    </div>`;
    if (box.innerHTML !== html) box.innerHTML = html;
  }

  private drawStart(): void {
    const lines = this.lines(this.options.store.world.peek());
    const option = this.chosenOption(lines);
    const line = lines.find((one) => option && one.options.includes(option));
    const said = this.input.value.trim() !== "" || (this.tray?.count ?? 0) > 0;
    const filesNowhere = !!option?.lent && (this.tray?.files ?? 0) > 0 && !this.filesOn;
    this.start.disabled = this.busy || !option || !line || !this.startable(option, line) || !said || filesNowhere;
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
    this.drawStart();
    this.note.hidden = true;
    remember("pick", option.key);
    remember("agent", option.agent);
    const colleaguesHome = this.colleagues && this.colleaguesHome !== undefined ? this.colleaguesHome : null;
    const world = this.options.store.world.peek();
    const login = this.loginFor(world, option);
    if (login && !option.lent) remember("account", `${login.hub}/${login.account}`);
    if (this.model) remember(`model:${option.agent}`, this.model);
    if (this.effort) remember(`effort:${option.agent}`, this.effort);
    const { words } = this.options;
    try {
      if (this.tray?.count) {
        this.start.textContent = words.sendingFiles;
        try {
          await this.tray.sendAll();
        } catch (error) {
          throw new NotSent(String(error));
        }
      }
      this.start.textContent = words.starting;
      const address = await this.options.begin({
        agent: option.agent,
        machine: option.machine,
        town: this.town,
        text,
        model: this.model,
        effort: this.effort,
        attachments: (this.tray?.arrived ?? []) as Attachment[],
        account: login ? { hub: login.hub, account: login.account } : null,
        worksOn: option.lent ? null : this.worksOn,
        colleagues: this.colleagues,
        home: option.home,
        colleaguesHome,
        filesOn: option.lent && (this.tray?.files ?? 0) > 0 ? this.filesOn : null,
      });
      this.input.value = "";
      this.tray?.take();
      this.draft = null;
      this.busy = false;
      this.dialog.close();
      this.options.opened(address);
    } catch (error) {
      this.busy = false;
      const why = error instanceof Error ? error.message : String(error);
      this.note.textContent = error instanceof NotSent ? words.filesNotSent : words.notOpened(why);
      this.note.title = error instanceof NotSent ? "" : why;
      this.note.hidden = false;
    } finally {
      this.start.textContent = this.options.words.start;
      this.drawStart();
    }
  }
}

/** Which of several logins stays chosen: the one chosen, while it can
 *  start; else the one used last there, else the default one, else the
 *  first that can. Undefined where there is no choice to make. */
function settle(choices: Choice[], chosen: string | null | undefined, key: string): string | null | undefined {
  if (!choices.length) return undefined;
  const can = (value: string | null | undefined) =>
    value !== undefined && choices.some((choice) => choice.value === (value ?? "") && !choice.disabled);
  if (can(chosen)) return chosen;
  const kept = remembered(key);
  if (kept !== null && can(kept || null)) return kept || null;
  if (can(null)) return null;
  const first = choices.find((choice) => !choice.disabled);
  return first ? first.value || null : undefined;
}

/** A file attached to the first words did not reach the machine; the tray
 *  says which, and why. */
class NotSent extends Error {}
