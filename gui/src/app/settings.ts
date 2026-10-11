// The settings: how the interface looks and reads on this device, and what
// this device is in the World. Choices take effect at once and are kept on
// this device; the language takes effect as the interface starts again.

import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import { chooseSize, chooseTheme, chosenSize, chosenTheme, textSizes, themes, type Theme } from "../theme";
import type { KeptNow, Legacy, Setup, WorldNow } from "../world/bridge";
import { RenewalRow, type Renewing } from "./renewal";
import { AppUpdateRow, type Updating } from "./app-update";
import { PhoneRow, type PhoneSeam } from "./phone";
import { desktopAvailable } from "../world/desktop";
import { chooseNotifying, notifying } from "./presence";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import type { SettingsWords } from "./words";
import { RemovalSection, type Removing } from "./removal";
import type { RemovalWords } from "./removal-words";

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
    /** How many days this computer keeps a command's output once its end
     *  was delivered; none before its node first ran. */
    retention?: { read(): Promise<number | null>; write(days: number): Promise<void> } | undefined;
  } | undefined;
  /** Told of a newer app version, so the sidebar can mark the way to it. */
  appUpdate?(version: string | null): void;
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
  private appUpdate: AppUpdateRow | null = null;
  private device: HTMLElement;
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
        <section class="home-section"><h2 class="section-title">${escape(words.device)}</h2><dl class="facts device"></dl><dl class="facts settings-service" hidden></dl><dl class="facts settings-renewal" hidden></dl><dl class="facts settings-app" hidden></dl><dl class="facts settings-phone" hidden></dl><div class="look settings-retention" hidden></div></section>
        <section class="home-section"><h2 class="section-title">${escape(words.keys)}</h2><dl class="facts keys">${words.keyList
          .map(([keys, what]) => `<dt>${keys.split(" ").map((key) => `<kbd>${escape(key)}</kbd>`).join(" ")}</dt><dd>${escape(what)}</dd>`)
          .join("")}</dl></section>
        ${options.computer ? `<section class="home-section"><h2 class="section-title">${escape(options.removalWords.title)}</h2><div class="removal"></div></section>` : ""}
      </div>`;
    this.device = this.element.querySelector<HTMLElement>(".device")!;
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
    this.askRetention();
    if (options.phone) new PhoneRow(this.element.querySelector<HTMLElement>(".settings-phone")!, words, options.phone);
    if (options.computer) new RemovalSection(this.element.querySelector<HTMLElement>(".removal")!, options.removalWords, options.computer.removing);
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
        case "language":
          options.chooseLanguage(value as "zh" | "en");
          return;
        case "notify":
          chooseNotifying(value === "on");
          options.notifyChanged?.(value === "on");
          break;
        case "retention":
          this.keepFor(Number(value));
          return;
      }
      redrawKeepingFocus(this.element, () => this.drawLook());
    });
    this.stop = effect(() => this.drawDevice(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
    this.renewal?.leave();
    this.appUpdate?.leave();
  }

  private retained: number | null = null;
  private retentionSaid = "";

  private askRetention(): void {
    const retention = this.options.computer?.retention;
    if (!retention) return;
    retention.read().then(
      (days) => {
        this.retained = days;
        this.drawRetention();
      },
      (error: unknown) => console.warn("the retention settings could not be read", error),
    );
  }

  /** The days to choose from: a day, a few, a week, two, a month, and
   *  whatever was set otherwise. */
  private drawRetention(): void {
    const { words } = this.options;
    const place = this.element.querySelector<HTMLElement>(".settings-retention")!;
    if (this.retained === null) return;
    const days = [...new Set([1, 3, 7, 14, 30, this.retained])].sort((a, b) => a - b);
    place.hidden = false;
    place.innerHTML = `<div class="choice-row" role="radiogroup" aria-label="${escape(words.retention)}"><span class="choice-label">${escape(words.retention)}</span><div class="choice-options">${days
      .map(
        (n) =>
          `<button type="button" class="choice${n === this.retained ? " chosen" : ""}" role="radio" aria-checked="${n === this.retained}" data-choice="retention" data-value="${n}">${escape(words.days(n))}</button>`,
      )
      .join("")}</div></div>${this.retentionSaid ? `<p class="settings-said">${escape(this.retentionSaid)}</p>` : ""}`;
  }

  private keepFor(days: number): void {
    const retention = this.options.computer?.retention;
    if (!retention || days === this.retained) return;
    const before = this.retained;
    this.retained = days;
    this.retentionSaid = "";
    redrawKeepingFocus(this.element, () => this.drawRetention());
    retention.write(days).catch((error: unknown) => {
      this.retained = before;
      this.retentionSaid = String(error);
      this.drawRetention();
    });
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

  /** Installs the service an installation would have, when it is missing:
   *  the rights were declined, or fastctx came here another way. */
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
    const button =
      kept === false
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
    const facts: [string, string, string?][] = [
      [words.machine, world.here],
      ...(hubs.length ? hubs : [[words.link, words.alone] as [string, string]]),
      [words.version, world.nodes.facts?.version ?? ""],
    ];
    this.device.innerHTML = facts
      .filter(([, value]) => value)
      .map(([name, value, why]) => `<dt>${escape(name)}</dt><dd${why ? ` title="${escape(why)}"` : ""}>${escape(value)}</dd>`)
      .join("");
  }
}
