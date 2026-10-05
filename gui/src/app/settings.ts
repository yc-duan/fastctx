// The settings: how the interface looks and reads on this device, and what
// this device is in the World. Choices take effect at once and are kept on
// this device; the language takes effect as the interface starts again.

import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import { chooseSize, chooseTheme, chosenSize, chosenTheme, textSizes, themes, type Theme } from "../theme";
import type { KeptNow, Legacy, Setup, WorldNow } from "../world/bridge";
import { RenewalRow, type Renewing } from "./renewal";
import { PhoneRow, type PhoneSeam } from "./phone";
import { desktopAvailable } from "../world/desktop";
import { chooseNotifying, notifying } from "./presence";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import type { SettingsWords } from "./words";

export interface SettingsOptions {
  store: Store;
  words: SettingsWords;
  head(title: string, actions?: string): string;
  language: "zh" | "en";
  chooseLanguage(language: "zh" | "en"): void;
  /** Says a moment as the front page does: how long ago. */
  ago(at: Date, now: Date): string;
  /** This computer, where the shell can set it up: where it stands, and
   *  installing or removing the service that keeps its node. */
  computer?: {
    setup(): Promise<Setup>;
    keep(): Promise<KeptNow>;
    release(): Promise<KeptNow>;
    /** Which World it runs, and putting a new one in place. */
    renewing: Renewing;
  } | undefined;
  /** This is an Android phone: what it lets the World do in the background. */
  phone?: PhoneSeam | undefined;
  /** Whether calls are wanted changed, for where calls are made by more
   *  than the page. */
  notifyChanged?(on: boolean): void;
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
  private device: HTMLElement;
  private service: HTMLElement;
  private setup: Setup | null = null;
  private armed = false;
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
        <section class="home-section"><h2 class="section-title">${escape(words.device)}</h2><dl class="facts device"></dl><dl class="facts settings-service" hidden></dl><dl class="facts settings-renewal" hidden></dl><dl class="facts settings-phone" hidden></dl></section>
        <section class="home-section"><h2 class="section-title">${escape(words.keys)}</h2><dl class="facts keys">${words.keyList
          .map(([keys, what]) => `<dt>${keys.split(" ").map((key) => `<kbd>${escape(key)}</kbd>`).join(" ")}</dt><dd>${escape(what)}</dd>`)
          .join("")}</dl></section>
      </div>`;
    this.device = this.element.querySelector<HTMLElement>(".device")!;
    this.service = this.element.querySelector<HTMLElement>(".settings-service")!;
    this.drawLook();
    this.askService();
    const renewing = options.computer?.renewing;
    if (renewing) this.renewal = new RenewalRow(this.element.querySelector<HTMLElement>(".settings-renewal")!, words, renewing);
    if (options.phone) new PhoneRow(this.element.querySelector<HTMLElement>(".settings-phone")!, words, options.phone);
    this.service.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-service]");
      if (button && !button.disabled) this.serve(button.dataset.service as "keep" | "release");
    });
    // A removal waits for a second press; it is let go when the reader
    // moves on.
    this.service.addEventListener("focusout", () => {
      if (this.armed) {
        this.armed = false;
        this.drawService();
      }
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
    this.stop = effect(() => this.drawDevice(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
    this.renewal?.leave();
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

  private serve(act: "keep" | "release"): void {
    const service = this.options.computer;
    if (!service || this.busy) return;
    if (act === "release" && !this.armed) {
      this.armed = true;
      this.drawService();
      this.service.querySelector<HTMLButtonElement>("[data-service]")?.focus();
      return;
    }
    this.armed = false;
    this.busy = true;
    this.said = "";
    this.drawService();
    const { words } = this.options;
    (act === "keep" ? service.keep() : service.release())
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
    const button =
      kept === null
        ? ""
        : kept
          ? `<button class="text-button quiet-button${this.armed ? " armed" : ""}" type="button" data-service="release"${this.busy ? " disabled" : ""}>${escape(this.armed ? words.releaseAgain : words.release)}</button>`
          : `<button class="text-button quiet-button" type="button" data-service="keep" title="${escape(words.keepAsks)}"${this.busy ? " disabled" : ""}>${escape(words.keep)}</button>`;
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
      row("size", words.textSize, [["", words.system], ...textSizes.map((size): [string, string] => [size, words.sizes[size]!])], chosenSize() ?? ""),
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
    const link = world.nodes.link;
    const linked =
      link.kind === "alone"
        ? words.alone
        : link.kind === "online"
          ? words.online(link.hub, this.options.ago(new Date(link.since), now))
          : link.kind === "connecting"
            ? words.connecting(link.last_error)
            : words.stopped(link.reason);
    const here = world.nodes.machines.find((node) => node.name === world.here);
    const version = here && "Ok" in here.facts && here.facts.Ok ? here.facts.Ok.version : "";
    const facts: [string, string][] = [
      [words.machine, world.here],
      [words.world, world.nodes.world ?? words.alone],
      [words.link, linked],
      [words.version, version],
    ];
    this.device.innerHTML = facts
      .filter(([, value]) => value)
      .map(([name, value]) => `<dt>${escape(name)}</dt><dd>${escape(value)}</dd>`)
      .join("");
  }
}
