// The settings: how the interface looks and reads on this device, what this
// device is in the World, and the keys, a tab each, the one last open kept
// on this device. Choices take effect at once and are kept on this device;
// the language takes effect as the interface starts again.

import { ProxyRow, type ProxySeam } from "./proxy-row";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import { chooseMotion, chosenMotion, motions, type Motion } from "../motion";
import { chooseSize, chooseTheme, chosenSize, chosenTheme, textSizes, themes, type Theme } from "../theme";
import type { KeptNow, Legacy, Setup, WorldNow } from "../world/bridge";
import type { Renewing } from "./renewal";
import { UpdateLine } from "./update-line";
import type { Updates } from "./updates";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdateWords } from "./update-words";
import { PhoneRow, type PhoneSeam } from "./phone";
import { desktopAvailable } from "../world/desktop";
import { chooseNotifying, notifying } from "./presence";
import type { Page } from "./shell";
import { bytesSaid } from "./data-page";
import { effect } from "./signals";
import type { Store } from "./store";
import type { SettingsWords } from "./words";
import type { Removing } from "./removal";
import { RemovalSheet } from "./removal-sheet";
import type { RemovalWords } from "./removal-words";
import { loginsOf } from "../world/accounts";
import type { NameProblem } from "../world/bridge";
import type { MachinePageWords } from "./machine-page-words";
import type { NameSaid } from "./name-words";
import { formerNames } from "./map-of";
import { link } from "./places";
import { inNoNetwork, machinesOf } from "./reach";
import { RenameField, renamerOf } from "./rename";
import { hubLine, type HubReach } from "./hub-line";

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
    /** The dialog that takes fastctx off, where another page opens it too. */
    removal?: RemovalSheet | undefined;
    /** Which World it runs, and putting a new one in place. */
    renewing: Renewing;
    /** How much World keeps on this computer, in bytes. */
    dataSize?(): Promise<number>;
    /** The proxy it reaches its hubs through. */
    proxy?: ProxySeam;
  } | undefined;
  /** New versions of the application: its rows, and the card a new one
   *  opens in; `relays` where this device passes packages to the network. */
  /** This device's updates: said in one line, done in their card. */
  updates?: { updates: Updates; words: UpdateWords; view: UpdateViewWords; open(): void } | undefined;
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

/** The settings' tabs, in their order. */
const TABS = ["look", "device", "keys"] as const;
type Tab = (typeof TABS)[number];

/** Where this device keeps the tab last open. */
const TAB_KEY = "settings.tab";

function keptTab(): Tab {
  try {
    const kept = localStorage.getItem(TAB_KEY);
    // Where there is no keyboard, its keys are not offered.
    if (kept === "keys" && matchMedia("(hover: none) and (pointer: coarse)").matches) return "look";
    return TABS.find((tab) => tab === kept) ?? "look";
  } catch {
    return "look";
  }
}

export class SettingsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private stop: () => void;
  private updateLine: UpdateLine | null = null;
  private device: HTMLElement;
  /** This computer's name: the way to its page, and renaming it there. */
  private name: HTMLElement;
  private renaming: RenameField | null = null;
  private service: HTMLElement;
  private setup: Setup | null = null;
  private busy = false;
  private said = "";
  private tab: Tab = "look";

  /** Shows `tab`'s sheet, and keeps it for the next time where `kept`. */
  private showTab(tab: Tab, kept = false): void {
    this.tab = tab;
    for (const button of this.element.querySelectorAll<HTMLElement>(".st-tab")) {
      const chosen = button.dataset.tab === tab;
      button.setAttribute("aria-selected", String(chosen));
      button.tabIndex = chosen ? 0 : -1;
    }
    for (const pane of this.element.querySelectorAll<HTMLElement>(".st-pane")) pane.hidden = pane.dataset.pane !== tab;
    this.element.querySelector<HTMLElement>(".st-sheet")?.classList.toggle("first", tab === TABS[0]);
    this.element.querySelector<HTMLElement>(".st-sheet")?.scrollTo({ top: 0 });
    if (!kept) return;
    try {
      localStorage.setItem(TAB_KEY, tab);
    } catch {
      // Kept for this visit only.
    }
  }

  constructor(private readonly options: SettingsOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page";
    this.element.innerHTML = `
      ${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}
      <div class="st-tabs" role="tablist" aria-label="${escape(words.title)}">${TABS.map((tab) => `<button class="st-tab" type="button" role="tab" id="st-tab-${tab}" aria-controls="st-pane-${tab}" data-tab="${tab}">${escape(words[tab])}</button>`).join("")}</div>
      <div class="settings st-room"><div class="st-sheet scroller">
        <section class="st-pane" id="st-pane-look" role="tabpanel" aria-labelledby="st-tab-look" data-pane="look"><div class="look"></div></section>
        <section class="st-pane settings-device" id="st-pane-device" role="tabpanel" aria-labelledby="st-tab-device" data-pane="device"><dl class="facts settings-name" hidden></dl><dl class="facts device"></dl><dl class="facts settings-app" hidden></dl><dl class="facts settings-service" hidden></dl>${options.computer ? `<dl class="facts settings-data"><dt>${escape(words.data)}</dt><dd><a class="settings-link" href="#/data"><span class="settings-data-size">${escape(words.dataOpen)}</span>${glyph("chevron", 14)}</a></dd></dl>` : ""}<dl class="facts settings-phone" hidden></dl>${options.computer ? `<div class="st-removal"><button class="text-button danger-text settings-uninstall" type="button" data-uninstall>${glyph("logout", 16)}<span>${escape(options.removalWords.open)}</span></button></div>` : ""}</section>
        <section class="st-pane settings-keys" id="st-pane-keys" role="tabpanel" aria-labelledby="st-tab-keys" data-pane="keys"><dl class="facts keys">${words.keyList
          .map(([keys, what]) => `<dt>${keys.split(" ").map((key) => `<kbd>${escape(key)}</kbd>`).join(" ")}</dt><dd>${escape(what)}</dd>`)
          .join("")}</dl></section>
      </div></div>`;
    this.showTab(keptTab());
    const tabs = this.element.querySelector<HTMLElement>(".st-tabs")!;
    tabs.addEventListener("click", (event) => {
      const tab = (event.target as HTMLElement).closest<HTMLElement>("[data-tab]")?.dataset.tab as Tab | undefined;
      if (tab) this.showTab(tab, true);
    });
    // The arrow keys go from tab to tab, as a row of tabs does.
    tabs.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      const at = TABS.indexOf(this.tab);
      const next = TABS[(at + (event.key === "ArrowRight" ? 1 : TABS.length - 1)) % TABS.length]!;
      event.preventDefault();
      this.showTab(next, true);
      tabs.querySelector<HTMLElement>(`[data-tab="${next}"]`)?.focus();
    });
    void options.computer?.dataSize?.().then(
      (bytes) => {
        const size = this.element.querySelector(".settings-data-size");
        if (size) size.textContent = bytesSaid(bytes);
      },
      () => {},
    );
    this.device = this.element.querySelector<HTMLElement>(".device")!;
    this.name = this.element.querySelector<HTMLElement>(".settings-name")!;
    // The proxy to the hubs follows the computer's name.
    if (options.computer?.proxy) this.name.after(new ProxyRow(words, options.computer.proxy).element);
    this.name.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-rename]")) this.rename();
    });
    this.service = this.element.querySelector<HTMLElement>(".settings-service")!;
    this.drawLook();
    this.askService();
    const updates = options.updates;
    if (updates) {
      this.updateLine = new UpdateLine(
        this.element.querySelector<HTMLElement>(".settings-app")!,
        updates.updates,
        updates.words,
        updates.view,
        words,
        options.computer?.renewing ?? null,
        updates.open,
      );
    }
    if (options.phone) new PhoneRow(this.element.querySelector<HTMLElement>(".settings-phone")!, words, options.phone);
    const computer = options.computer;
    if (computer) {
      // Uninstalling is a dialog of its own: the two ways, what each takes
      // and leaves, and its steps ticked off, over the settings.
      const sheet = computer.removal ?? new RemovalSheet(options.removalWords, computer.removing);
      this.element.querySelector("[data-uninstall]")!.addEventListener("click", () => sheet.open());
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
    this.updateLine?.leave();
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
    const formerly = former.length && renaming ? ` title="${escape(renaming.words.formerly(former))}"` : "";
    // In no network the computer has no details page: its name is only its
    // name, renamed here.
    const name = inNoNetwork(world)
      ? `<span${formerly}>${escape(here.written)}</span>`
      : `<a class="settings-link" href="${link({ kind: "machine", name: here.written })}"${formerly}><span>${escape(here.written)}</span>${glyph("chevron", 14)}</a>`;
    const html = `<dt>${escape(this.options.words.machine)}</dt><dd class="settings-name-value">${name}</dd>${
      renamable ? `<dd class="sd-act"><button class="text-button quiet-button" type="button" data-rename>${escape(renaming!.words.rename)}</button></dd>` : ""
    }`;
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
    // The field takes the row, its own buttons in place of renaming.
    this.name.querySelector(".sd-act")?.remove();
    value.classList.add("sd-wide");
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
        ? `<dd class="sd-act"><button class="text-button quiet-button" type="button" data-service="keep" title="${escape(words.keepAsks)}"${this.busy ? " disabled" : ""}>${escape(words.keep)}</button></dd>`
        : "";
    this.service.hidden = false;
    const legacy = this.setup?.legacy ?? null;
    const old = legacySaid(legacy, words);
    // What became of an old fastctx in a word, the sentence behind it.
    const done = legacy?.kind === "unreadable" ? words.legacyUnreadable(legacy.why) : legacy?.waiting ? words.legacyBusy(legacy.waiting) : words.legacyDone;
    this.service.innerHTML = `<dt>${escape(words.service)}</dt><dd><span${kept ? ` title="${escape(words.serviceWhat)}"` : ""}>${escape(state)}</span>${this.said ? `<p class="settings-said">${escape(this.said)}</p>` : ""}</dd>${button}${
      old ? `<dt>${escape(words.legacy)}</dt><dd class="settings-legacy"><span title="${escape(old)}">${escape(done)}</span></dd>` : ""
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
    const { words } = this.options;
    if (!world) {
      this.device.innerHTML = `<p class="quiet">${escape(words.notInShell)}</p>`;
      return;
    }
    // How this computer or device reaches each hub it is logged in to, and
    // as whom: drawn, the hub's name and the accounts beside the line, the
    // address and since when said when pointed at.
    const hubs = world.nodes.hubs.map((hub, index) => {
      const link = hub.link;
      const label = `<dt>${index === 0 ? escape(words.link) : ""}</dt>`;
      if (link.kind === "alone") return `${label}<dd>${escape(words.alone)}</dd>`;
      const reach: HubReach = link.kind === "online" ? "online" : link.kind === "connecting" ? "connecting" : "stopped";
      const said =
        link.kind === "online"
          ? words.online(link.hub, this.options.ago(new Date(link.since), now))
          : link.kind === "connecting"
            ? words.connecting(link.last_error)
            : link.dissolved
              ? words.dissolved
              : words.stopped(link.reason);
      const why = link.kind === "connecting" ? (link.last_error ?? "") : link.kind === "stopped" ? link.reason : "";
      const logins = hub.logins.join(", ");
      const pointed = [said, words.as(logins), why].filter(Boolean).join("\n");
      return `${label}<dd class="sd-hub" title="${escape(pointed)}">${hubLine(reach, hub.name)}<span class="sd-hub-name">${escape(hub.name)}</span>${
        reach === "online" ? "" : `<span class="sd-state">${escape(said)}</span>`
      }${logins ? `<span class="sd-login">${glyph("person", 14)}<span>${escape(logins)}</span></span>` : ""}</dd>`;
    });
    // The version is said once, by the local service's own row below; a
    // computer's name has its own row above.
    this.device.innerHTML = [
      ...(this.options.phone ? [`<dt>${escape(words.phoneName)}</dt><dd>${escape(world.here)}</dd>`] : []),
      ...(hubs.length ? hubs : [`<dt>${escape(words.link)}</dt><dd>${escape(words.alone)}</dd>`]),
    ].join("");
  }
}
