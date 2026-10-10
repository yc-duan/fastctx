// Beginning a conversation (`world-gui.md`, 界面的结构): the machine it lives
// on, the account it works for where more than one may open it there, which
// agent, and which of the agent's logins there when it has several, which
// model and how hard it thinks, which project, the machine it works on (the
// one it lives on unless another is chosen, `world-run.md`, 这里), whether it
// may ask for colleagues, where they would live and as which Codex login
// there, and the first words. A sheet over the page, opened by the button in the sidebar or
// Ctrl+N; on a phone it rises from the foot of the screen. Only machines
// online now are offered to live on, only projects of the account that span
// the machine chosen, only machines the account may work on or use agents
// on, and only agents its own programs say can start there; their models
// are those the program lists. Colleagues are asked for nowhere unless a
// machine is chosen for them. Once the agent is asked in, its conversation
// opens and its first turn is watched from the start.
//
// Files may come with the first words. They wait in the sheet while the
// machine, agent and project are still being chosen, and go when the
// conversation is begun: the first makes a place on the chosen machine for
// the agent about to be asked in, the rest join it there, and the agent
// starts in that place with all of them.

import { AttachTray, type AttachSource, type AttachWords } from "../conversation/attach";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import type { HubAccount, Login } from "../world/accounts";
import type { AgentFacts, AgentModel, WorldAgents } from "../world/agents";
import { accountOf, homeChoice, homeLabel, homesOf, installed, loggedOut, versionOf, type HomeWords } from "./agent-homes";
import type { ConsentHome } from "./claude-consent";
import type { Attachment, UploadTarget } from "../world/attachments";
import type { WorldNow } from "../world/bridge";
import type { Address } from "../world/events";
import { colleagueHomes, groupsOf, loginLabel, machinesOf, openersOn, workPlaces, type Machine } from "./reach";
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
  /** The account it works for, where the user chose among several. */
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
}

export interface NewTalkOptions {
  store: Store;
  words: NewTalkWords;
  homeWords: HomeWords;
  /** What the machine's own programs say of the agents on it. */
  agents(machine: string): Promise<WorldAgents>;
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

const known = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
];

/** What is known of the agents on one machine. */
type Asked = { state: "asking" } | { state: "known"; facts: AgentFacts[] } | { state: "unknown"; why: string };

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

/** The login of `agent` written `home`, or where none is chosen or it
 *  cannot start, the default one, else the first that can. */
function pick(facts: AgentFacts[], agent: string, home: string | null | undefined): AgentFacts | null {
  const homes = homesOf(facts, agent);
  const chosen = home === undefined ? undefined : homes.find((f) => homeChoice(f) === home && usable(f));
  return chosen ?? homes.find(usable) ?? homes[0] ?? null;
}

export class NewTalk {
  private readonly dialog: HTMLDialogElement;
  private readonly form: HTMLFormElement;
  private readonly input: HTMLTextAreaElement;
  private readonly note: HTMLElement;
  private readonly start: HTMLButtonElement;
  private machine: string | null = null;
  private agent: string = remembered("agent") ?? "claude";
  private town: string | null = null;
  private model: string | null = null;
  private effort: string | null = null;
  /** The account chosen, as `hub/account`. */
  private account: string | null = remembered("account");
  /** Where it works and where its colleagues live, as written; null for
   *  the machine it lives on, and for no colleagues. */
  private worksOn: string | null = null;
  /** It cannot work where it lives, so where it works must be chosen. */
  private placeNeeded = false;
  private colleagues: string | null = null;
  /** The agent's login chosen, as its home's directory or null for the
   *  default one; undefined until one is, when the one used last on that
   *  machine is taken. */
  private home: string | null | undefined = undefined;
  /** The same for its colleagues' Codex where they live. */
  private colleaguesHome: string | null | undefined = undefined;
  private asked = new Map<string, Asked>();
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
        case "machine":
          this.machine = value;
          this.worksOn = null;
          this.colleagues = null;
          this.home = undefined;
          this.colleaguesHome = undefined;
          this.ask(value);
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
        case "agent":
          this.agent = value;
          this.home = undefined;
          this.model = null;
          this.effort = null;
          break;
        case "home":
          this.home = value || null;
          this.model = null;
          this.effort = null;
          break;
        case "model":
          this.model = value;
          this.effort = null;
          break;
        case "effort":
          this.effort = value;
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
          const machine = this.machine!;
          const chosen = this.chosen();
          const target: UploadTarget =
            this.draft?.for === chosen
              ? { kind: "draft", resident: this.draft.resident, town: this.town }
              : { kind: "new", agent: this.agent, town: this.town };
          const done = await attach.upload(machine, target, source, progress);
          this.draft = { resident: done.owner.resident, for: chosen };
          return done;
        },
      },
      () => this.drawStart(),
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
    return JSON.stringify([this.machine, this.account, this.agent, this.town]);
  }

  /** What the chosen machine said of its agents, once it has. */
  private known(machine: string | null = this.machine): AgentFacts[] | null {
    const asked = machine ? this.asked.get(machine) : undefined;
    return asked?.state === "known" ? asked.facts : null;
  }

  /** The login the chosen agent would start in on the chosen machine. */
  private login(): AgentFacts | null {
    const facts = this.known();
    return facts ? pick(facts, this.agent, this.home) : null;
  }

  /** Opens the sheet; `town` is the project to begin in, when opened from
   *  one. */
  open(town: string | null = null): void {
    const world = this.options.store.world.peek();
    const online = this.machines(world).filter((choice) => !choice.disabled);
    if (!this.machine || !online.some((choice) => choice.value === this.machine)) {
      const here = world?.here ?? null;
      this.machine = online.find((choice) => choice.value === here)?.value ?? online[0]?.value ?? null;
    }
    this.town = town;
    this.model = null;
    this.effort = null;
    this.worksOn = null;
    this.colleagues = null;
    this.home = undefined;
    this.colleaguesHome = undefined;
    this.note.hidden = true;
    // What is on each machine is asked again each time: a program may have
    // been installed, logged in or updated since.
    this.asked.clear();
    if (this.machine) this.ask(this.machine);
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

  /** Asks a machine what agents it has, once per opening of the sheet. */
  private ask(machine: string): void {
    if (this.asked.has(machine)) return;
    this.asked.set(machine, { state: "asking" });
    this.options
      .agents(machine)
      .then((world) => {
        const answer = world.machines.find((m) => (m.kind === "answered" ? m.reply.machine : m.machine) === machine);
        this.asked.set(
          machine,
          answer?.kind === "answered"
            ? { state: "known", facts: answer.reply.agents }
            : { state: "unknown", why: answer?.kind === "no_answer" ? answer.reason : answer?.kind === "offline" ? this.options.words.offline : "" },
        );
      })
      .catch((error: unknown) => this.asked.set(machine, { state: "unknown", why: String(error) }))
      .finally(() => {
        // The machine to live on, or the one colleagues would live on.
        if (this.dialog.open) this.draw();
      });
  }

  private machines(world: WorldNow | null): Choice[] {
    const { words } = this.options;
    // A machine an agent may live on: one whose agents some login may use.
    return machinesOf(world)
      .filter((machine) => machine.here || openersOn(world, machine).length)
      .map((machine) => ({
        value: machine.name,
        label: machine.written,
        note: machine.here ? words.here : machine.online === false ? words.offline : undefined,
        disabled: !machine.here && machine.online !== true,
      }));
  }

  /** The machine chosen to live on, as this computer reaches it. */
  private lives(world: WorldNow | null): Machine | null {
    return machinesOf(world).find((machine) => machine.name === this.machine) ?? null;
  }

  /** The logins that may open a conversation on the machine chosen, and
   *  the one chosen of them: the only one, or the one picked. */
  private openers(world: WorldNow | null): { all: Login[]; chosen: Login | null } {
    const lives = this.lives(world);
    const all = lives ? openersOn(world, lives) : [];
    const key = (login: Login) => `${login.hub}/${login.account}`;
    const chosen = all.find((login) => key(login) === this.account) ?? all[0] ?? null;
    return { all, chosen };
  }

  /** The agents on the chosen machine as choices: whether each can start,
   *  and its version, as its own program says. Unknown is not a no. */
  private agents(): Choice[] {
    const { words } = this.options;
    const asked = this.machine ? this.asked.get(this.machine) : undefined;
    return known.map((agent) => {
      if (!asked || asked.state === "asking") return { ...agent, note: words.asking };
      if (asked.state === "unknown") return { ...agent, note: words.unsure };
      const homes = homesOf(asked.facts, agent.value);
      if (!homes.length) return { ...agent, note: words.unsure };
      if (!homes.some(installed)) return { ...agent, note: words.notInstalled, disabled: true };
      if (!homes.some(usable)) return { ...agent, note: words.loggedOut, disabled: true };
      const facts = pick(asked.facts, agent.value, agent.value === this.agent ? this.home : undefined)!;
      const version = versionOf(facts) ?? undefined;
      if (facts.unattended?.kind === "known" && !facts.unattended.value) {
        return { ...agent, note: version ? `${version} · ${words.needsConsent}` : words.needsConsent };
      }
      return { ...agent, note: version };
    });
  }

  /** The logins of `agent` on `machine` as choices, when it has more than
   *  one: who is logged in to each and on what plan. */
  private logins(machine: string | null, agent: string): Choice[] {
    const { words, homeWords } = this.options;
    const facts = this.known(machine);
    const homes = facts ? homesOf(facts, agent).filter(installed) : [];
    if (homes.length < 2) return [];
    return homes.map((home) => {
      const plan = accountOf(home).plan;
      const consent = agent === "claude" && home.unattended?.kind === "known" && !home.unattended.value ? words.needsConsent : null;
      return {
        value: homeChoice(home) ?? "",
        label: homeLabel(home, homeWords),
        note: loggedOut(home) ? words.loggedOut : [plan, consent].filter(Boolean).join(" · ") || undefined,
        disabled: loggedOut(home),
      };
    });
  }

  /** The models the chosen agent's program lists in the chosen login. */
  private models(): AgentModel[] {
    const facts = this.login();
    return facts?.models.kind === "known" ? facts.models.value : [];
  }

  private draw(): void {
    const { words, store } = this.options;
    const world = store.world.peek();
    const machines = this.machines(world);
    const lives = this.lives(world);
    const logins = this.openers(world);
    const login = logins.chosen;
    // The projects of the account it works for that span where it lives.
    const group = login ? groupsOf(world).find((group) => group.login.hub === login.hub && group.login.account === login.account) : null;
    const towns: Choice[] = [
      { value: "", label: words.noProject },
      ...(world?.towns ?? [])
        .filter((town) => (!this.machine || town.record.machines.includes(this.machine)) && (!group || group.towns.includes(town.town)))
        .map((town) => ({ value: town.town, label: town.record.name })),
    ];
    const places: Choice[] = lives
      ? workPlaces(world, lives, login).map((machine) => ({
          value: machine.written === lives.written ? "" : machine.written,
          label: machine.written === lives.written ? words.sameMachine : machine.written,
          note: machine.written !== lives.written && machine.online === false ? words.offline : undefined,
        }))
      : [];
    if (this.worksOn && !places.some((choice) => choice.value === this.worksOn)) this.worksOn = null;
    // An account lent only the agents where it lives has it work elsewhere.
    this.placeNeeded = !!lives && !places.some((choice) => choice.value === "");
    const colleagueMachines = lives ? colleagueHomes(world, lives, login) : [];
    const homes: (Choice & { machine?: string })[] = lives
      ? [
          { value: "", label: words.noColleagues },
          ...colleagueMachines.map((machine) => ({
            value: machine.written,
            label: machine.written,
            machine: machine.name,
            note: machine.here ? words.here : machine.online === false ? words.offline : undefined,
          })),
        ]
      : [];
    if (this.colleagues && !homes.some((choice) => choice.value === this.colleagues)) this.colleagues = null;
    // Where colleagues live, the Codex login they use there, when there
    // are several.
    const colleaguesOn = colleagueMachines.find((machine) => machine.written === this.colleagues)?.name ?? null;
    if (colleaguesOn) this.ask(colleaguesOn);
    const colleagueLogins = colleaguesOn ? this.logins(colleaguesOn, "codex") : [];
    this.colleaguesHome = settle(colleagueLogins, this.colleaguesHome, `home:${colleaguesOn}:codex`);
    if (this.town && !towns.some((choice) => choice.value === this.town)) this.town = null;
    const agents = this.agents();
    // An agent that cannot start here gives way to one that can.
    if (agents.find((choice) => choice.value === this.agent)?.disabled) {
      const usable = agents.find((choice) => !choice.disabled);
      if (usable) {
        this.agent = usable.value;
        this.home = undefined;
      }
    }
    const agentLogins = this.logins(this.machine, this.agent);
    this.home = settle(agentLogins, this.home, `home:${this.machine}:${this.agent}`);
    const models = this.models();
    const model =
      models.find((m) => m.model === this.model) ??
      models.find((m) => m.model === remembered(`model:${this.agent}`)) ??
      models.find((m) => m.is_default) ??
      models[0] ??
      null;
    this.model = model?.model ?? null;
    const efforts = model?.reasoning_efforts.map((e) => e.effort) ?? [];
    const effort =
      [this.effort, remembered(`effort:${this.agent}`), model?.default_reasoning_effort].find((e) => e && efforts.includes(e)) ?? null;
    this.effort = effort;
    const row = (kind: string, label: string, choices: (Choice & { machine?: string })[], chosen: string | null, none = words.noMachine) =>
      `<div class="choice-row" role="radiogroup" aria-label="${escape(label)}">
        <span class="choice-label">${escape(label)}</span>
        <div class="choice-options">${
          choices.length
            ? choices
                .map(
                  (choice) =>
                    `<button type="button" class="choice${choice.value === (chosen ?? "") ? " chosen" : ""}" role="radio" aria-checked="${choice.value === (chosen ?? "")}" data-choice="${kind}" data-value="${escape(choice.value)}"${choice.machine ? ` data-machine="${escape(choice.machine)}"` : ""}${choice.disabled ? " disabled" : ""}>${escape(choice.label)}${choice.note ? `<span class="choice-note">${escape(choice.note)}</span>` : ""}</button>`,
                )
                .join("")
            : `<span class="choice-none">${escape(none)}</span>`
        }</div>
      </div>`;
    this.dialog.querySelector(".choices")!.innerHTML = [
      row("machine", words.machine, machines, this.machine),
      logins.all.length > 1
        ? row(
            "account",
            words.account,
            logins.all.map((one) => ({ value: `${one.hub}/${one.account}`, label: loginLabel(one) })),
            login ? `${login.hub}/${login.account}` : null,
          )
        : "",
      row("agent", words.agent, agents, this.agent),
      agentLogins.length ? row("home", words.login, agentLogins, this.home ?? "") : "",
      models.length
        ? row(
            "model",
            words.model,
            models.map((m) => ({ value: m.model, label: m.display_name, note: m.is_default ? words.defaultModel : undefined })),
            this.model,
          )
        : "",
      efforts.length > 1 ? row("effort", words.effort, efforts.map((e) => ({ value: e, label: e })), this.effort) : "",
      row("town", words.project, towns, this.town),
      places.length > 1 || this.placeNeeded ? row("works", words.worksOn, places, this.worksOn ?? "") : "",
      this.placeNeeded && lives ? `<p class="choice-said">${escape(words.lentOnly(lives.written))}</p>` : "",
      homes.length > 1 ? row("colleagues", words.colleagues, homes, this.colleagues ?? "") : "",
      colleagueLogins.length ? row("colleagues-home", words.colleaguesLogin, colleagueLogins, this.colleaguesHome ?? "") : "",
      this.colleagues ? `<p class="choice-said">${escape(words.colleaguesWhat(this.colleagues))}</p>` : "",
    ].join("");
    // What was sent went for another machine, agent or project: it is sent
    // again, to the one chosen now, when the conversation begins.
    if (this.draft && this.draft.for !== this.chosen()) {
      this.draft = null;
      this.tray?.reset();
    }
    this.drawStart();
  }

  private drawStart(): void {
    const chosen = this.agents().find((choice) => choice.value === this.agent);
    const said = this.input.value.trim() !== "" || (this.tray?.count ?? 0) > 0;
    this.start.disabled = this.busy || !this.machine || !!chosen?.disabled || !said || (this.placeNeeded && !this.worksOn);
  }

  /** Claude starts on a machine only once the user has let it work
   *  without asking there: asked now, when it is needed. Not knowing is
   *  not a no; Claude Code then says for itself. */
  private async allowed(machine: string): Promise<boolean> {
    const facts = this.login();
    if (facts?.unattended?.kind !== "known" || facts.unattended.value) return true;
    const several = this.logins(machine, "claude").length > 0;
    const home: ConsentHome = { dir: homeChoice(facts), label: several ? homeLabel(facts, this.options.homeWords) : null };
    if (!this.options.allowClaude || !(await this.options.allowClaude(machine, this.options.words.allowAndStart, home))) return false;
    facts.unattended = { kind: "known", value: true };
    this.draw();
    return true;
  }

  private async submit(): Promise<void> {
    const text = this.input.value.trim();
    if (this.busy || !this.machine || this.start.disabled || (!text && !this.tray?.count)) return;
    if (this.agent === "claude" && !(await this.allowed(this.machine))) return;
    this.busy = true;
    this.drawStart();
    this.note.hidden = true;
    remember("agent", this.agent);
    const home = this.home === undefined ? null : this.home;
    const colleaguesHome = this.colleagues && this.colleaguesHome !== undefined ? this.colleaguesHome : null;
    if (this.home !== undefined) remember(`home:${this.machine}:${this.agent}`, this.home ?? "");
    const login = this.openers(this.options.store.world.peek()).chosen;
    if (login) remember("account", `${login.hub}/${login.account}`);
    if (this.model) remember(`model:${this.agent}`, this.model);
    if (this.effort) remember(`effort:${this.agent}`, this.effort);
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
        agent: this.agent,
        machine: this.machine,
        town: this.town,
        text,
        model: this.model,
        effort: this.effort,
        attachments: (this.tray?.arrived ?? []) as Attachment[],
        account: login ? { hub: login.hub, account: login.account } : null,
        worksOn: this.worksOn,
        colleagues: this.colleagues,
        home,
        colleaguesHome,
      });
      this.input.value = "";
      this.tray?.take();
      this.draft = null;
      this.busy = false;
      this.dialog.close();
      this.options.opened(address);
    } catch (error) {
      this.busy = false;
      this.note.textContent =
        error instanceof NotSent ? words.filesNotSent : words.notOpened(error instanceof Error ? error.message : String(error));
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
