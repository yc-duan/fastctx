// What the workbench brings that has settings of its own (`world-layers.md`):
// history and model collaboration, tabs of the conversations' settings, and
// the sentinels, reached from Settings. Each is a page: off, the film that tells
// what it does, playing on a loop, and the button that turns it on under
// it; turned on, a page over the window says it is on and what the
// interface gained. On, its page shows a still of the film, to be watched
// again over the page, beside what it gave; the settings below; and, last
// and light, the way to turn it off, asked once more before it is.
//
// Model collaboration is drawn from its own file (`collab-feature.ts`), as
// history is.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { loginsOf } from "../world/accounts";
import type { WorldAgents } from "../world/agents";
import type { HomeWords } from "./agent-homes";
import type { FeaturesWords } from "./features-words";
import { layerOpened } from "./back";
import { CollabFeature } from "./collab-feature";
import type { Confirm } from "./confirm";
import type { Feature } from "./places";
import { inNoNetwork } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import { leave } from "./leave";
import { collaboration } from "../film/collaboration";
import { sentinelFeature } from "./sentinel-feature";
import type { SentinelSeam } from "../world/sentinels";
import { Film } from "../film/film";
import type { Built, Shape } from "../film/reel";
import { HistoryFeature, type FeatureFrame } from "./history-feature";
import { watchFilm } from "../film/theater";

/** Whether model collaboration can be turned on; a node offers a colleague
 *  only as `collab.rs` lets it. */
const COLLABORATION_READY = true;

export interface FeaturesOptions {
  store: Store;
  words: FeaturesWords;
  homeWords: HomeWords;
  english: boolean;
  head(title: string, actions?: string): string;
  feature: Exclude<Feature, "conversations">;
  agents(): Promise<WorldAgents>;
  /** Asks before a feature is turned off. */
  confirm: Confirm;
  /** The sentinels' settings, and the machines they may live on. */
  sentinels: { seam: SentinelSeam; machines(hub: string, account: string): { name: string; form: "desktop" | "laptop" | "server" }[] };
}


export class FeaturesPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private stop: () => void = () => {};
  /** The films on the page, stopped when it is drawn again or left. */
  private films: Film[] = [];
  /** Ends the film watched over the page, while one is. */
  private unwatch: (() => void) | null = null;
  /** Takes away the page that says a feature was turned on, while it is up. */
  private undone: (() => void) | null = null;

  constructor(private readonly options: FeaturesOptions) {
    const { words, feature } = options;
    this.title = feature === "sentinels" ? words.sentinels.name : feature === "history" ? words.history.name : words.collaboration.name;
    this.element = document.createElement("div");
    this.element.className = "features-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`)}<div class="features-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".features-body")!;
    this.body.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-act='replay']")) this.replay();
    });
    if (feature === "sentinels") this.sentinels();
    else if (feature === "history") this.history();
    else this.collaboration();
  }

  leave(): void {
    this.stop();
    this.dropFilms();
    this.unwatch?.();
    this.undone?.();
  }

  /** A film set into `holder`: playing, or `still` on its last frame. A
   *  still is a band over the feature's settings, wide even on a phone,
   *  so that they begin on the first screen. */
  private film(holder: Element | null, make: (shape: Shape) => Built, label: string, still = false): void {
    if (!holder) return;
    const film = new Film(make, label, still, still ? "wide" : undefined);
    this.films.push(film);
    holder.replaceChildren(film.element);
  }

  private dropFilms(): void {
    for (const film of this.films) film.destroy();
    this.films = [];
  }

  /** The film over the page, playing, until the page behind is touched
   *  or Escape is pressed. */
  private watch(make: (shape: Shape) => Built, label: string): void {
    this.unwatch?.();
    const close = watchFilm(make, label, this.options.words.closeFilm);
    this.unwatch = () => {
      close();
      this.unwatch = null;
    };
  }

  /** The page that says a feature was turned on, the same for every one,
   *  over the whole window: it is on, what the interface gained, and the way
   *  back to its page, which Esc and the system back take as well. */
  private celebrate(name: string, gained: string, make: (shape: Shape) => Built): void {
    const { words } = this.options;
    this.undone?.();
    const done = document.createElement("div");
    done.className = "feature-done";
    done.setAttribute("role", "dialog");
    done.setAttribute("aria-modal", "true");
    done.setAttribute("aria-label", words.enabled(name));
    // The strip at the top moves the window, as a page's head does.
    done.innerHTML = `<header class="page-head feature-done-head"></header>
      <div class="feature-done-box"><div class="feature-done-art"></div>
        <h2 class="feature-done-title">${escape(words.enabled(name))}</h2>
        <p class="feature-done-gained">${escape(gained)}</p>
        <button class="sheet-go feature-done-back" type="button">${escape(words.back)}</button></div>`;
    const film = new Film(make, name, true);
    done.querySelector(".feature-done-art")!.append(film.element);
    let open = true;
    const takeAway = () => {
      if (!open) return;
      open = false;
      removeEventListener("keydown", key);
      this.undone = null;
      leave(done, () => {
        film.destroy();
        done.remove();
      });
    };
    const close = () => {
      if (!open) return;
      takeAway();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close();
    };
    done.querySelector(".feature-done-back")!.addEventListener("click", close);
    addEventListener("keydown", key);
    document.body.append(done);
    layerOpened({ isOpen: () => open, close });
    this.undone = takeAway;
    done.querySelector<HTMLButtonElement>(".feature-done-back")!.focus({ preventScroll: true });
  }

  /** The top of a feature's page while it is on: a still of its film,
   *  watched again over the page, beside what it gave; a feature with no
   *  settings (`solo`) holds them in the middle of the page, as its film is
   *  while it is off. */
  private onTop(gained: string, solo = false, more = ""): string {
    const { words } = this.options;
    return `<div class="feature-on${solo ? " solo" : ""}">
      <button class="feature-still" type="button" data-act="replay" aria-label="${escape(words.replay)}"></button>
      <div class="feature-on-said">
        <span class="tag feature-tag-on">${glyph("check", 12)}<span>${escape(words.on)}</span></span>
        <p class="feature-on-gained">${escape(gained)}</p>
        <button class="text-button quiet-button" type="button" data-act="replay">${glyph("regenerate", 14)}<span>${escape(words.replay)}</span></button>${more}
      </div>
    </div>`;
  }

  /** The way to turn a feature off: last on its page, and light, so it is
   *  never taken for the way back. */
  private offWay(name: string): string {
    return `<div class="feature-off-way"><button class="text-button quiet-button feature-turn-off" type="button" data-act="turn-off">${escape(this.options.words.turnOffNamed(name))}</button></div>`;
  }

  /** Asks before `name` is turned off, saying what folds away and what
   *  carries on; `off` turns it off, and fails as the World says. */
  private askOff(name: string, what: string, off: () => Promise<void>, done: () => void): void {
    const { words, confirm } = this.options;
    confirm.ask({ title: words.offTitle(name), what, go: words.turnOff, gentle: true, act: off, done });
  }

  /** The login the account's settings are read and kept for. */
  private login(): { hub: string; account: string } | null {
    const [first] = loginsOf(this.options.store.world.peek()?.accounts);
    return first ? { hub: first.hubName, account: first.account } : null;
  }

  private sentinels(): void {
    const { store, words, english, sentinels } = this.options;
    this.stop = sentinelFeature(this.body, store, words, english, sentinels.seam, sentinels.machines);
  }

  /** History: a switch alone (`history-feature.ts`), kept for the account,
   *  or for this computer while it is in no network. */
  private history(): void {
    const { store, words } = this.options;
    let drawn = false;
    this.stop = effect(() => {
      const world = store.world.get();
      if (drawn || !world) return;
      drawn = true;
      const feature = new HistoryFeature(this.frame(), inNoNetwork(world) ? null : (this.login() ?? "none"));
      this.replayFilm = () => this.watch(feature.film, words.history.name);
    });
  }

  /** What a feature drawn from its own file borrows of this page. */
  private frame(): FeatureFrame {
    return {
      body: this.body,
      words: this.options.words,
      film: (holder, make, label, still) => this.film(holder, make, label, still),
      dropFilms: () => this.dropFilms(),
      celebrate: (name, gained, make) => this.celebrate(name, gained, make),
      onTop: (gained, solo) => this.onTop(gained, solo),
      offWay: (name) => this.offWay(name),
      askOff: (name, what, off, done) => this.askOff(name, what, off, done),
      gives: (list) => this.gives(list),
    };
  }

  /** Model collaboration (`collab-feature.ts`): the account's presets and
   *  callers, or in no network the ways into one. */
  private collaboration(): void {
    const { store, words, english, homeWords, agents, confirm } = this.options;
    if (!COLLABORATION_READY) {
      const make = (shape: Shape) => collaboration(words.collaboration.film, shape);
      this.replayFilm = () => this.watch(make, words.collaboration.name);
      // Its story and what it gives, and that it is still being made.
      this.body.classList.add("features-film");
      this.body.innerHTML = `<div class="feature-intro with-gives"><div class="feature-film"></div>${this.gives()}<p class="feature-coming">${glyph("info", 18)}<span>${escape(words.coming)}</span></p></div>`;
      this.film(this.body.querySelector(".feature-film"), make, words.collaboration.name);
      return;
    }
    const feature = new CollabFeature(this.frame(), { store, english, homeWords, agents, confirm });
    this.replayFilm = () => this.watch(feature.film, words.collaboration.name);
    this.stop = feature.stop;
  }

  /** Watches the page's film again, over the page. */
  private replayFilm: (() => void) | null = null;

  private replay(): void {
    this.replayFilm?.();
  }

  /** What model collaboration gives the user: a ready text that teaches
   *  the calling model how to ask for what it wants, and nothing to
   *  operate by hand. */
  private gives(list = this.options.words.collaboration.gives): string {
    return `<ul class="feature-gives">${list
      .map(([mark, line]) => `<li><span class="feature-give-mark">${glyph(mark, 16)}</span><span>${escape(line)}</span></li>`)
      .join("")}</ul>`;
  }
}
