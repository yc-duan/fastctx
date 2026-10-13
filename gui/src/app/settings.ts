// The settings: how the interface looks and reads on this device, and what
// this device is in the World. Choices take effect at once and are kept on
// this device; the language takes effect as the interface starts again.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import { chooseMotion, chosenMotion, motions, type Motion } from "../motion";
import { chooseSize, chooseTheme, chosenSize, chosenTheme, textSizes, themes, type Theme } from "../theme";
import type { KeptNow, Legacy, Setup, WorldNow } from "../world/bridge";
import { Sheet } from "./confirm";
import { RenewalRow, type Renewing } from "./renewal";
import { AppUpdateRow, type Updating } from "./app-update";
import { PhoneRow, type PhoneSeam } from "./phone";
import { desktopAvailable } from "../world/desktop";
import { chooseNotifying, notifying } from "./presence";
import type { Page } from "./shell";
import { bytesSaid } from "./data-page";
import { effect } from "./signals";
import type { Store } from "./store";
import type { SettingsWords } from "./words";
import { RemovalSection, type Removing } from "./removal";
import type { RemovalWords } from "./removal-words";
import { loginsOf } from "../world/accounts";
import type { NameProblem } from "../world/bridge";
import type { MachinePageWords } from "./machine-page-words";
import type { NameSaid } from "./name-words";
import { formerNames } from "./map-of";
import { link } from "./places";
import { machinesOf } from "./reach";
import { RenameField, renamerOf } from "./rename";

export interface SettingsOptions {
  store: Store;
  words: SettingsWords;
  removalWords: RemovalWords;
  head(title: string, actions?: string): string;
  language: "zh" | "en";
  chooseLanguage(language: "zh" | "en"): void;
  /** Says a moment as the front page does: how long ago. */
  ago(at: Date, now: Date): string;
  /** This computer, where the shell can set it up: where it stands,
   *  installing the service that keeps its node when it is missing, and
   *  taking fastctx off it. */
  computer?: {
    setup(): Promise<Setup>;
    keep(): Promise<KeptNow>;
    removing: Removing;
    /** Which World it runs, and putting a new one in place. */
    renewing: Renewing;
    /** The app's own version, and a newer one from its releases. */
    updating?: Updating | undefined;
    /** How much World keeps on this computer, in bytes. */
    dataSize?(): Promise<number>;
  } | undefined;
  /** Told of a newer app version, so the sidebar can mark the way to it. */
  appUpdate?(version: string | null): void;
  /** This is an Android phone: what it lets the World do in the background. */
  phone?: PhoneSeam | undefined;
  /** Whether calls are wanted changed, for where calls are made by more
   *  than the page. */
  notifyChanged?(on: boolean): void;
  /** Renaming this computer: the words of its field, the rule a name is
   *  checked by, and the renaming. */
  renaming?: {
    words: MachinePageWords;
    nameSaid: NameSaid;
    check(text: string): Promise<NameProblem | null>;
    here(to: string): Promise<void>;
  } | undefined;
}

/** What was done with what an old fastctx left, in a sentence; nothing
 *  when there was nothing to do. */
export function legacySaid(legacy: Legacy | null, words: SettingsWords): string {
  if (!legacy) return "";
  if (legacy.kind === "unreadable") return words.legacyUnreadable(legacy.why);
  if (!legacy.tidied && !legacy.archived && !legacy.waiting) return "";
  return words.legacyCleaned(legacy.tidied, legacy.archived, legacy.archives.at(-1) ?? legacy.record, legacy.waiting);
}

export class SettingsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private stop: () => void;
  private renewal: RenewalRow | null = null;
  private appUpdate: AppUpdateRow | null = null;
  private device: HTMLElement;
  /** This computer's name: the way to its page, and renaming it there. */
  private name: HTMLElement;
  private renaming: RenameField | null = null;
  private service: HTMLElement;
  private setup: Setup | null = null;
  private busy = false;
  private said = "";

  constructor(private readonly options: SettingsOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page";
    this.element.innerHTML = `
      ${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}
      <div class="settings scroller">
        <section class="home-section"><h2 class="section-title">${escape(words.look)}</h2><div class="look"></div></section>
        <section class="home-section settings-device"><h2 class="section-title">${escape(words.device)}</h2><dl class="facts settings-name" hidden></dl><dl class="facts device"></dl><dl class="facts settings-renewal" hidden></dl><dl class="facts settings-app" hidden></dl><dl class="facts settings-service" hidden></dl>${options.computer ? `<dl class="facts settings-data"><dt>${escape(words.data)}</dt><dd><a class="settings-link" href="#/data"><span class="settings-data-size">${escape(words.dataOpen)}</span>${glyph("chevron", 14)}</a></dd></dl>` : ""}<dl class="facts settings-phone" hidden></dl></section>
        <section class="home-section settings-keys"><h2 class="section-title">${escape(words.keys)}</h2><dl class="facts keys">${words.keyList
          .map(([keys, what]) => `<dt>${keys.split(" ").map((key) => `<kbd>${escape(key)}</kbd>`).join(" ")}</dt><dd>${escape(what)}</dd>`)
          .join("")}</dl></section>
        ${options.computer ? `<section class="home-section"><h2 class="section-title">${escape(options.removalWords.title)}</h2><button class="text-button danger-text settings-uninstall" type="button" data-uninstall>${glyph("logout", 16)}<span>${escape(options.removalWords.open)}</span></button></section>` : ""}
      </div>`;
    void options.computer?.dataSize?.().then(
      (bytes) => {
        const size = this.element.querySelector(".settings-data-size");
        if (size) size.textContent = bytesSaid(bytes);
      },
      () => {},
    );
    this.device = this.element.querySelector<HTMLElement>(".device")!;
    this.name = this.element.querySelector<HTMLElement>(".settings-name")!;
    this.name.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-rename]")) this.rename();
    });
    this.service = this.element.querySelector<HTMLElement>(".settings-service")!;
    this.drawLook();
    this.askService();
    const renewing = options.computer?.renewing;
    if (renewing) this.renewal = new RenewalRow(this.element.querySelector<HTMLElement>(".settings-renewal")!, words, renewing);
    const updating = options.computer?.updating;
    if (updating) {
      this.appUpdate = new AppUpdateRow(
        this.element.querySelector<HTMLElement>(".settings-app")!,
        words,
        updating,
        (version) => options.appUpdate?.(version),
      );
    }
    if (options.phone) new PhoneRow(this.element.querySelector<HTMLElement>(".settings-phone")!, words, options.phone);
    const computer = options.computer;
    if (computer) {
      // Uninstalling is a dialog of its own: the two ways, what each takes
      // and leaves, and its steps ticked off, over the settings.
      let sheet: Sheet | null = null;
      this.element.querySelector("[data-uninstall]")!.addEventListener("click", () => {
        if (!sheet) {
          const opened = new Sheet("removal-sheet");
          opened.body.innerHTML = `${opened.head(options.removalWords.title, options.removalWords.cancel)}<div class="removal"></div>`;
          new RemovalSection(opened.body.querySelector<HTMLElement>(".removal")!, options.removalWords, computer.removing, (on) => (opened.busy = on));
          sheet = opened;
        }
        sheet.show();
      });
    }
    this.service.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-service]");
      if (button && !button.disabled) this.keep();
    });
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-choice]");
      if (!button) return;
      const value = button.dataset.value!;
      switch (button.dataset.choice) {
        case "theme":
          chooseTheme(value ? (value as Theme) : null);
          break;
        case "size":
          chooseSize(value || null);
          break;
        case "motion":
          chooseMotion(value as Motion);
          break;
        case "language":
          options.chooseLanguage(value as "zh" | "en");
          return;
        case "notify":
          chooseNotifying(value === "on");
          options.notifyChanged?.(value === "on");
          break;
      }
      redrawKeepingFocus(this.element, () => this.drawLook());
    });
    this.stop = effect(() => {
      const world = options.store.world.get();
      this.drawName(world);
      this.drawDevice(world, options.store.clock.get());
    });
  }

  leave(): void {
    this.stop();
    this.renewal?.leave();
    this.appUpdate?.leave();
    this.renaming?.dispose();
  }

  /** This computer's name, which opens its page, the names it had behind
   *  the pointer, and renaming it where its hubs rename. */
  private drawName(world: WorldNow | null): void {
    if (this.renaming) return;
    const here = machinesOf(world).find((machine) => machine.here);
    const renaming = this.options.renaming;
    this.name.hidden = !here || !world;
    if (!here || !world) return;
    const former = formerNames(world, here);
    const renamable = !!renaming && !!renamerOf(here, world, async () => {}, renaming.here);
    const html = `<dt>${escape(this.options.words.machine)}</dt><dd class="settings-name-value"><a class="settings-link" href="${link({ kind: "machine", name: here.written })}"${
      former.length && renaming ? ` title="${escape(renaming.words.formerly(former))}"` : ""
    }><span>${escape(here.written)}</span>${glyph("chevron", 14)}</a>${
      renamable ? `<button class="text-button quiet-button" type="button" data-rename>${escape(renaming!.words.rename)}</button>` : ""
    }</dd>`;
    if (this.name.innerHTML !== html) this.name.innerHTML = html;
  }

  private rename(): void {
    const renaming = this.options.renaming;
    const world = this.options.store.world.peek();
    const here = machinesOf(world).find((machine) => machine.here);
    if (!renaming || !world || !here || this.renaming) return;
    const value = this.name.querySelector<HTMLElement>(".settings-name-value");
    if (!value) return;
    const end = () => {
      field.dispose();
      this.renaming = null;
      this.name.innerHTML = "";
      this.drawName(this.options.store.world.peek());
    };
    const field = new RenameField({
      words: renaming.words,
      nameSaid: renaming.nameSaid,
      name: here.name,
      check: (text) => renaming.check(text),
      rename: async (to) => {
        await renaming.here(to);
        await this.options.store.refresh();
      },
      done: end,
      cancel: end,
    });
    this.renaming = field;
    value.replaceChildren(field.element);
    field.focus();
  }

  private askService(): void {
    const computer = this.options.computer;
    if (!computer) return;
    computer.setup().then(
      (setup) => {
        this.setup = setup;
        this.drawService();
      },
      (error: unknown) => {
        this.said = String(error);
        this.drawService();
      },
    );
  }

  /** Installs the service joining or creating a network would have, when it
   *  is missing: the rights were declined, or fastctx came here another way. */
  private keep(): void {
    const service = this.options.computer;
    if (!service || this.busy) return;
    this.busy = true;
    this.said = "";
    this.drawService();
    const { words } = this.options;
    service
      .keep()
      .then(
        (done) => {
          this.said = done.kind === "command" ? words.command(done.command) : words.kept[done.kind];
        },
        (error: unknown) => {
          this.said = String(error);
        },
      )
      .finally(() => {
        this.busy = false;
        this.askService();
      });
  }

  private drawService(): void {
    const { words } = this.options;
    const kept = this.setup ? this.setup.service : null;
    if (kept === null && !this.said) return;
    const state = kept === null ? "" : kept ? words.serviceOn : words.serviceOff;
    // The service belongs to a computer in a network (`world-gui.md`, 设置):
    // one used on its own is not offered it.
    const inNetwork = loginsOf(this.options.store.world.peek()?.accounts).length > 0;
    const button =
      kept === false && inNetwork
        ? `<button class="text-button quiet-button" type="button" data-service="keep" title="${escape(words.keepAsks)}"${this.busy ? " disabled" : ""}>${escape(words.keep)}</button>`
        : "";
    this.service.hidden = false;
    const old = legacySaid(this.setup?.legacy ?? null, words);
    this.service.innerHTML = `<dt>${escape(words.service)}</dt><dd><span>${escape(state)}</span>${button}${this.said ? `<p class="settings-said">${escape(this.said)}</p>` : ""}</dd>${
      old ? `<dt>${escape(words.legacy)}</dt><dd class="settings-legacy">${escape(old)}</dd>` : ""
    }`;
  }

  private drawLook(): void {
    const { words, language } = this.options;
    const row = (kind: string, label: string, choices: [string, string][], chosen: string) =>
      `<div class="choice-row" role="radiogroup" aria-label="${escape(label)}"><span class="choice-label">${escape(label)}</span><div class="choice-options">${choices
        .map(
          ([value, name]) =>
            `<button type="button" class="choice${value === chosen ? " chosen" : ""}" role="radio" aria-checked="${value === chosen}" data-choice="${kind}" data-value="${escape(value)}">${escape(name)}</button>`,
        )
        .join("")}</div></div>`;
    this.element.querySelector(".look")!.innerHTML = [
      row("theme", words.theme, [["", words.system], ...themes.map((theme): [string, string] => [theme, words.themes[theme]])], chosenTheme() ?? ""),
      row("size", words.textSize, [["", words.sizeDefault], ...textSizes.map((size): [string, string] => [size, words.sizes[size]!])], chosenSize() ?? ""),
      row("motion", words.motion, motions.map((motion): [string, string] => [motion, words.motions[motion]]), chosenMotion()),
      row("language", words.language, [["zh", "中文"], ["en", "English"]], language),
      // Only where the shell can call the user from outside the window.
      desktopAvailable() || this.options.phone
        ? row("notify", words.notify, [["on", words.notifyOn], ["off", words.notifyOff]], notifying() ? "on" : "off")
        : "",
    ].join("");
  }

  private drawDevice(world: WorldNow | null, now: Date): void {
    const { words, store } = this.options;
    if (!store.live || !world) {
      this.device.innerHTML = `<p class="quiet">${escape(words.notInShell)}</p>`;
      return;
    }
    // How this computer or device reaches each hub it is logged in to, and
    // as whom.
    const hubs = world.nodes.hubs.map((hub): [string, string, string] => {
      const link = hub.link;
      const linked =
        link.kind === "alone"
          ? words.alone
          : link.kind === "online"
            ? words.online(link.hub, this.options.ago(new Date(link.since), now))
            : link.kind === "connecting"
              ? words.connecting(link.last_error)
              : words.stopped(link.reason);
      const why = link.kind === "connecting" ? (link.last_error ?? "") : link.kind === "stopped" ? link.reason : "";
      return [words.hub(hub.name), [linked, words.as(hub.logins.join(", "))].filter(Boolean).join(" · "), why];
    });
    // The version is said once, by the local service's own row below; a
    // computer's name has its own row above.
    const facts: [string, string, string?][] = [
      ...(this.options.phone ? [[words.phoneName, world.here] as [string, string]] : []),
      ...(hubs.length ? hubs : [[words.link, words.alone] as [string, string]]),
    ];
    this.device.innerHTML = facts
      .filter(([, value]) => value)
      .map(([name, value, why]) => `<dt>${escape(name)}</dt><dd${why ? ` title="${escape(why)}"` : ""}>${escape(value)}</dd>`)
      .join("");
  }
}
