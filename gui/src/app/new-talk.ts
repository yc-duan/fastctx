// Beginning a conversation: which machine, which agent on it, which
// project, which model and how hard it thinks, and the first words
// (`world-gui.md`, 界面的结构). A sheet over the page, opened by the button in
// the sidebar or Ctrl+N; on a phone it rises from the foot of the screen.
// Only machines online now are offered, only projects that span the machine
// chosen, and only agents its own programs say can start there; their
// models are those the program lists. Once the agent is asked in, its
// conversation opens and its first turn is watched from the start.
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
import type { AgentFacts, AgentModel, WorldAgents } from "../world/agents";
import type { Attachment, UploadTarget } from "../world/attachments";
import type { WorldNow } from "../world/bridge";
import type { Address } from "../world/events";
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
}

export interface NewTalkOptions {
  store: Store;
  words: NewTalkWords;
  /** What the machine's own programs say of the agents on it. */
  agents(machine: string): Promise<WorldAgents>;
  begin(begun: Begun): Promise<Address>;
  opened(address: Address): void;
  /** Asks the user to let Claude work without asking on a machine, where
   *  it has not yet; true once Claude Code there records it. */
  allowClaude?(machine: string, go: string): Promise<boolean>;
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

/** The program an agent would start as on a machine: of the ones found, one
 *  that is there. */
function factsFor(facts: AgentFacts[], agent: string): AgentFacts | null {
  const all = facts.filter((f) => f.agent === agent);
  return all.find((f) => f.program.kind === "known" && f.program.value?.exists) ?? all[0] ?? null;
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
          this.ask(value);
          break;
        case "agent":
          this.agent = value;
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

  /** The machine, agent and project the files go for. */
  private chosen(): string {
    return JSON.stringify([this.machine, this.agent, this.town]);
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
        if (this.dialog.open && this.machine === machine) this.draw();
      });
  }

  private machines(world: WorldNow | null): Choice[] {
    const { words } = this.options;
    // A phone is never where an agent lives.
    return (world?.nodes.machines ?? [])
      .filter((node) => !node.revoked && !node.device)
      .map((node) => ({
        value: node.name,
        label: node.name,
        note: node.name === world?.here ? words.here : node.online === false ? words.offline : undefined,
        disabled: node.online !== true,
      }));
  }

  /** The agents on the chosen machine as choices: whether each can start,
   *  and its version, as its own program says. Unknown is not a no. */
  private agents(): Choice[] {
    const { words } = this.options;
    const asked = this.machine ? this.asked.get(this.machine) : undefined;
    return known.map((agent) => {
      if (!asked || asked.state === "asking") return { ...agent, note: words.asking };
      if (asked.state === "unknown") return { ...agent, note: words.unsure };
      const facts = factsFor(asked.facts, agent.value);
      if (!facts) return { ...agent, note: words.unsure };
      if (facts.program.kind === "known" && (facts.program.value === null || !facts.program.value.exists)) {
        return { ...agent, note: words.notInstalled, disabled: true };
      }
      if (facts.login.kind === "known" && facts.login.value.kind === "logged_out") {
        return { ...agent, note: words.loggedOut, disabled: true };
      }
      const version = facts.version.kind === "known" ? /\d+(?:\.\d+)+/.exec(facts.version.value)?.[0] : undefined;
      if (facts.unattended?.kind === "known" && !facts.unattended.value) {
        return { ...agent, note: version ? `${version} · ${words.needsConsent}` : words.needsConsent };
      }
      return { ...agent, note: version };
    });
  }

  /** The models the chosen agent's program lists on the chosen machine. */
  private models(): AgentModel[] {
    const asked = this.machine ? this.asked.get(this.machine) : undefined;
    if (asked?.state !== "known") return [];
    const facts = factsFor(asked.facts, this.agent);
    return facts?.models.kind === "known" ? facts.models.value : [];
  }

  private draw(): void {
    const { words, store } = this.options;
    const world = store.world.peek();
    const machines = this.machines(world);
    const towns: Choice[] = [
      { value: "", label: words.noProject },
      ...(world?.towns ?? [])
        .filter((town) => !this.machine || town.record.machines.includes(this.machine))
        .map((town) => ({ value: town.town, label: town.record.name })),
    ];
    if (this.town && !towns.some((choice) => choice.value === this.town)) this.town = null;
    const agents = this.agents();
    // An agent that cannot start here gives way to one that can.
    if (agents.find((choice) => choice.value === this.agent)?.disabled) {
      const usable = agents.find((choice) => !choice.disabled);
      if (usable) this.agent = usable.value;
    }
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
    const row = (kind: string, label: string, choices: Choice[], chosen: string | null, none = words.noMachine) =>
      `<div class="choice-row" role="radiogroup" aria-label="${escape(label)}">
        <span class="choice-label">${escape(label)}</span>
        <div class="choice-options">${
          choices.length
            ? choices
                .map(
                  (choice) =>
                    `<button type="button" class="choice${choice.value === (chosen ?? "") ? " chosen" : ""}" role="radio" aria-checked="${choice.value === (chosen ?? "")}" data-choice="${kind}" data-value="${escape(choice.value)}"${choice.disabled ? " disabled" : ""}>${escape(choice.label)}${choice.note ? `<span class="choice-note">${escape(choice.note)}</span>` : ""}</button>`,
                )
                .join("")
            : `<span class="choice-none">${escape(none)}</span>`
        }</div>
      </div>`;
    this.dialog.querySelector(".choices")!.innerHTML = [
      row("machine", words.machine, machines, this.machine),
      row("agent", words.agent, agents, this.agent),
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
    this.start.disabled = this.busy || !this.machine || !!chosen?.disabled || !said;
  }

  /** Claude starts on a machine only once the user has let it work
   *  without asking there: asked now, when it is needed. Not knowing is
   *  not a no; Claude Code then says for itself. */
  private async allowed(machine: string): Promise<boolean> {
    const asked = this.asked.get(machine);
    const facts = asked?.state === "known" ? factsFor(asked.facts, "claude") : null;
    if (facts?.unattended?.kind !== "known" || facts.unattended.value) return true;
    if (!this.options.allowClaude || !(await this.options.allowClaude(machine, this.options.words.allowAndStart))) return false;
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

/** A file attached to the first words did not reach the machine; the tray
 *  says which, and why. */
class NotSent extends Error {}
