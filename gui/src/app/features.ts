// The features (`world-gui.md`, 功能): a card for each, and each its own
// page. A feature is off until turned on; off, its page is the film that
// tells what it does, playing on a loop, and the button that turns it on
// under it. Turned on, the same page for every feature fills the window: it
// is on, what the interface gained, and the way back to the features. On,
// its page shows a still of the film, to be watched again over the page,
// beside what it gave; the settings below; and, last and light, the way to
// turn it off, asked once more before it is.
//
// The workbench is the first: the agents' work drawn step by step, agents
// on other computers and in a phone; it is kept per device and has no
// settings. Model collaboration:
// for the whole account, which agents may hand a part of their work to a
// GPT (callers), which GPTs they may hand it to (callees: a Codex login
// each, its model and effort, the name and the line the calling model
// chooses by, whether it may use the screen), the text the callers are given
// and the rules every callee is given. Project memory: for the whole account, a
// project's journal and decisions shown with it and given to its
// conversations; it has no settings.

import { EffortSlider, effortLabel } from "../conversation/effort-slider";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentFacts, AgentModel, WorldAgents } from "../world/agents";
import { readCollab, sameAgentLogin, writeCollab, type AgentLogin, type Collab, type CollabView } from "../world/collab";
import { agentName, titleOf } from "../world/names";
import { accountOf, homeChoice, homeLabel, homesOf, listed, usableHome, type HomeWords } from "./agent-homes";
import type { FeaturesWords } from "./features-words";
import { layerOpened } from "./back";
import type { Confirm } from "./confirm";
import { link, type Feature } from "./places";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import { residentsOf, type Store } from "./store";
import { deviceFeatures } from "./device-features";
import { leave } from "./leave";
import { collaboration } from "../film/collaboration";
import { memoryFilm } from "../film/memory";
import { sentinelFilm } from "../film/sentinel";
import { SentinelForm } from "./sentinel-form";
import type { SentinelSeam, SentinelSettingsView } from "../world/sentinels";
import { Film } from "../film/film";
import type { Built, Shape } from "../film/reel";

/** Whether model collaboration can be turned on; a node offers a colleague
 *  only as `collab.rs` lets it. */
const COLLABORATION_READY = true;
import { watchFilm } from "../film/theater";
import { workbench } from "../film/workbench";

export interface FeaturesOptions {
  store: Store;
  words: FeaturesWords;
  homeWords: HomeWords;
  english: boolean;
  head(title: string, actions?: string): string;
  feature: Feature | null;
  agents(): Promise<WorldAgents>;
  /** Asks before a feature is turned off. */
  confirm: Confirm;
  /** Project memory's switch, for the account. */
  /** Project memory, on or off for a login's account; null lets the World pick. */
  memory: { enabled(hub: string | null, account: string | null): Promise<boolean>; set(on: boolean, hub: string | null, account: string | null): Promise<void> };
  /** The sentinels' settings, and the machines they may live on. */
  sentinels: { seam: SentinelSeam; machines(hub: string, account: string): { name: string; form: "desktop" | "laptop" | "server" }[] };
}

/** A login an agent can work in, as the settings name it, with what the
 *  page shows of it. */
interface Known {
  login: AgentLogin;
  label: string;
  models: AgentModel[];
}

export class FeaturesPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private stop: () => void = () => {};
  private account: Login | null = null;
  private view: CollabView | null = null;
  private draft: Collab | null = null;
  private problem: { text: string; detail: string } | null = null;
  private saved = false;
  private known: Known[] = [];
  private sliders: EffortSlider[] = [];
  /** The films on the page, stopped when it is drawn again or left. */
  private films: Film[] = [];
  /** Ends the film watched over the page, while one is. */
  private unwatch: (() => void) | null = null;
  /** Takes away the page that says a feature was turned on, while it is up. */
  private undone: (() => void) | null = null;

  constructor(private readonly options: FeaturesOptions) {
    const { words, feature } = options;
    this.title =
      feature === "collaboration"
        ? words.collaboration.name
        : feature === "memory"
          ? words.memory.name
          : feature === "sentinels"
            ? words.sentinels.name
            : feature === "conversations"
              ? words.conversations.name
              : words.title;
    this.element = document.createElement("div");
    this.element.className = "features-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`)}<div class="features-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".features-body")!;
    this.body.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-act='replay']")) this.replay();
    });
    if (feature === null) this.cards();
    else if (feature === "conversations") this.conversations();
    else if (feature === "memory") this.memory();
    else if (feature === "sentinels") this.sentinels();
    else this.collaboration();
  }

  leave(): void {
    this.stop();
    this.sliders = [];
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
   *  to the features, which Esc and the system back take as well. */
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
      location.hash = link({ kind: "features" });
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
  private onTop(gained: string, solo = false): string {
    const { words } = this.options;
    return `<div class="feature-on${solo ? " solo" : ""}">
      <button class="feature-still" type="button" data-act="replay" aria-label="${escape(words.replay)}"></button>
      <div class="feature-on-said">
        <span class="tag feature-tag-on">${glyph("check", 12)}<span>${escape(words.on)}</span></span>
        <p class="feature-on-gained">${escape(gained)}</p>
        <button class="text-button quiet-button" type="button" data-act="replay">${glyph("regenerate", 14)}<span>${escape(words.replay)}</span></button>
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

  /** Every feature, a card each. */
  private cards(): void {
    const { words } = this.options;
    const tag = (on: boolean | null) =>
      on === null ? "" : on ? `<span class="tag feature-tag-on">${glyph("check", 12)}<span>${escape(words.on)}</span></span>` : `<span class="tag feature-tag-off">${escape(words.off)}</span>`;
    const coming = `<span class="tag">${escape(words.coming)}</span>`;
    const draw = (on: boolean | null) => {
      this.body.innerHTML = `<ul class="feature-cards">
        <li><a class="feature-card" href="${link({ kind: "feature", feature: "conversations" })}">
          <span class="feature-card-art">${glyph("phone", 22)}${glyph("desktop", 22)}</span>
          <span class="feature-card-name">${escape(words.conversations.name)}</span>
          <span class="feature-card-line">${escape(words.conversations.line)}</span>${tag(deviceFeatures.conversations.peek())}</a></li>
        <li><a class="feature-card${COLLABORATION_READY ? "" : " coming"}" href="${link({ kind: "feature", feature: "collaboration" })}">
          <span class="feature-card-art">${glyph("codex", 22)}${glyph("claude", 22)}</span>
          <span class="feature-card-name">${escape(words.collaboration.name)}</span>
          <span class="feature-card-line">${escape(words.collaboration.line)}</span>${COLLABORATION_READY ? tag(on) : coming}</a></li>
        <li><a class="feature-card" href="${link({ kind: "feature", feature: "memory" })}">
          <span class="feature-card-art">${glyph("journal", 22)}${glyph("spec", 22)}</span>
          <span class="feature-card-name">${escape(words.memory.name)}</span>
          <span class="feature-card-line">${escape(words.memory.line)}</span>${tag(memoryOn)}</a></li>
        <li><a class="feature-card" href="${link({ kind: "feature", feature: "sentinels" })}">
          <span class="feature-card-art">${glyph("sentinel", 22)}${glyph("server", 22)}</span>
          <span class="feature-card-name">${escape(words.sentinels.name)}</span>
          <span class="feature-card-line">${escape(words.sentinels.line)}</span>${tag(sentinelsOn)}</a></li>
      </ul>`;
    };
    let memoryOn: boolean | null = null;
    let sentinelsOn: boolean | null = null;
    draw(null);
    const login = this.login();
    if (login)
      this.options.sentinels.seam.read(login.hub, login.account).then(
        (view) => {
          sentinelsOn = view.settings.home !== null;
          draw(collabOn);
        },
        () => {},
      );
    this.options.memory.enabled(login?.hub ?? null, login?.account ?? null).then(
      (on) => {
        memoryOn = on;
        draw(collabOn);
      },
      () => {},
    );
    let collabOn: boolean | null = null;
    if (!COLLABORATION_READY) return;
    // In no network, this computer keeps its own settings.
    const [first] = loginsOf(this.options.store.world.peek()?.accounts);
    readCollab(first?.hubName ?? null, first?.account ?? null)
      .then((view) => draw((collabOn = view.collab.on)))
      .catch(() => draw((collabOn = false)));
  }

  /** The workbench: its film and its switch; on, a still of the film
   *  waits at the top. */
  private conversations(): void {
    const { words } = this.options;
    const make = (shape: Shape) => workbench(words.conversations.film, shape);
    const label = words.conversations.name;
    this.replayFilm = () => this.watch(make, label);
    const draw = () => {
      this.dropFilms();
      const on = deviceFeatures.conversations.peek();
      this.body.classList.toggle("features-film", !on);
      this.body.classList.toggle("features-on", on);
      this.body.innerHTML = on
        ? this.onTop(words.conversations.after, true) + this.offWay(label)
        : `<div class="feature-intro"><div class="feature-film"></div><button class="sheet-go feature-turn" type="button" data-turn="1">${escape(words.turnOn)}</button></div>`;
      this.film(this.body.querySelector(on ? ".feature-still" : ".feature-film"), make, label, on);
    };
    draw();
    this.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-act='turn-off']")) {
        this.askOff(label, words.conversations.offWhat, () => deviceFeatures.setConversations(false), draw);
        return;
      }
      const turn = target.closest<HTMLButtonElement>("[data-turn]");
      if (!turn || turn.disabled) return;
      turn.disabled = true;
      void deviceFeatures.setConversations(true).then(
        () => {
          draw();
          this.celebrate(label, words.conversations.after, make);
        },
        (error: unknown) => {
          turn.disabled = false;
          turn.title = detailOf(error);
        },
      );
    });
  }

  /** Project memory: its film and its switch, for the account; on, a still
   *  of the film waits at the top. */
  private memory(): void {
    const { words } = this.options;
    const login = this.login();
    const memory = this.options.memory;
    const seam = {
      enabled: () => memory.enabled(login?.hub ?? null, login?.account ?? null),
      set: (on: boolean) => memory.set(on, login?.hub ?? null, login?.account ?? null),
    };
    const make = (shape: Shape) => memoryFilm(words.memory.film, shape);
    const label = words.memory.name;
    this.replayFilm = () => this.watch(make, label);
    const draw = (on: boolean) => {
      this.dropFilms();
      this.body.classList.toggle("features-film", !on);
      this.body.classList.toggle("features-on", on);
      this.body.innerHTML = on
        ? this.onTop(words.memory.after, true) + this.offWay(label)
        : `<div class="feature-intro"><div class="feature-film"></div><button class="sheet-go feature-turn" type="button" data-turn="1">${escape(words.turnOn)}</button></div>`;
      this.film(this.body.querySelector(on ? ".feature-still" : ".feature-film"), make, label, on);
    };
    seam.enabled().then(draw, () => draw(false));
    this.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-act='turn-off']")) {
        this.askOff(label, words.memory.offWhat, () => seam.set(false), () => draw(false));
        return;
      }
      const turn = target.closest<HTMLButtonElement>("[data-turn]");
      if (!turn || turn.disabled) return;
      turn.disabled = true;
      void seam.set(true).then(
        () => {
          draw(true);
          this.celebrate(label, words.memory.after, make);
        },
        (error: unknown) => {
          turn.disabled = false;
          turn.title = detailOf(error);
        },
      );
    });
  }

  /** The login the account's settings are read and kept for. */
  private login(): { hub: string; account: string } | null {
    const [first] = loginsOf(this.options.store.world.peek()?.accounts);
    return first ? { hub: first.hubName, account: first.account } : null;
  }

  /** Sentinels: their film and, on, their settings; on is a machine for
   *  them to live on, chosen and saved. */
  private sentinels(): void {
    const { words } = this.options;
    const label = words.sentinels.name;
    const make = (shape: Shape) => sentinelFilm(words.sentinels.film, shape);
    this.replayFilm = () => this.watch(make, label);
    const login = this.login();
    const seam = this.options.sentinels.seam;
    let view: SentinelSettingsView | null = null;
    const form = login
      ? new SentinelForm({
          words: words.sentinels,
          seam,
          login,
          machines: () => (login ? this.options.sentinels.machines(login.hub, login.account) : []),
          saved: (saved) => {
            const was = view?.settings.home ?? null;
            view = saved;
            if (was === null && saved.settings.home !== null) {
              draw(true);
              this.celebrate(label, words.sentinels.after, make);
            }
          },
        })
      : null;
    const draw = (on: boolean, starting = false) => {
      this.dropFilms();
      this.body.classList.toggle("features-film", !on && !starting);
      this.body.classList.toggle("features-on", on || starting);
      this.body.innerHTML =
        on || starting
          ? `${on ? this.onTop(words.sentinels.after) : ""}<div class="sf-holder"></div>${on ? this.offWay(label) : ""}`
          : `<div class="feature-intro"><div class="feature-film"></div><button class="sheet-go feature-turn" type="button" data-turn="1"${form ? "" : " disabled"}>${escape(words.turnOn)}</button></div>`;
      if (form && (on || starting)) {
        this.body.querySelector(".sf-holder")!.append(form.element);
        if (starting && !view?.settings.home) form.start();
        else if (view) form.show(view);
      }
      if (on) this.film(this.body.querySelector(".feature-still"), make, label, true);
      else if (!starting) this.film(this.body.querySelector(".feature-film"), make, label);
    };
    draw(false);
    if (login)
      seam.read(login.hub, login.account).then(
        (read) => {
          view = read;
          draw(read.settings.home !== null);
        },
        () => {},
      );
    this.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-turn]")) return draw(false, true);
      if (target.closest("[data-act='turn-off']") && login && view) {
        const current = view;
        this.askOff(
          label,
          words.sentinels.offWhat,
          async () => {
            const reply = await seam.write(login.hub, login.account, current.version, { ...current.settings, home: null });
            if (reply.kind === "conflict") throw new Error(words.sentinels.changedElsewhere);
            view = reply.view;
          },
          () => draw(false),
        );
      }
    });
  }

  private collaboration(): void {
    const { store, words } = this.options;
    const make = (shape: Shape) => collaboration(words.collaboration.film, shape);
    this.replayFilm = () => this.watch(make, words.collaboration.name);
    if (!COLLABORATION_READY) {
      // Its story and what it gives, and that it is still being made.
      this.body.classList.add("features-film");
      this.body.innerHTML = `<div class="feature-intro with-gives"><div class="feature-film"></div>${this.gives()}<p class="feature-coming">${glyph("info", 18)}<span>${escape(words.coming)}</span></p></div>`;
      this.film(this.body.querySelector(".feature-film"), make, words.collaboration.name);
      return;
    }
    const logins = loginsOf(store.world.peek()?.accounts);
    this.account = logins[0] ?? null;
    this.draw();
    void this.load();
    void this.ask();
    this.stop = effect(() => {
      store.world.get();
      this.drawNow();
    });
    this.body.addEventListener("click", (event) => this.click(event));
    this.body.addEventListener("input", (event) => this.input(event));
    this.body.addEventListener("change", (event) => this.input(event));
  }

  /** The account's settings, or in no network this computer's own. */
  private async load(): Promise<void> {
    const account = this.account;
    try {
      this.view = await readCollab(account?.hubName ?? null, account?.account ?? null);
      this.draft = structuredClone(this.view.collab);
      this.problem = null;
    } catch (error) {
      this.view = null;
      this.draft = null;
      this.problem = { text: this.options.words.unavailable, detail: detailOf(error) };
    }
    this.draw();
  }

  /** The logins of every agent the account may use, from what their
   *  machines say. */
  private async ask(): Promise<void> {
    try {
      const answer = await this.options.agents();
      const known: Known[] = [];
      for (const machine of answer.machines) {
        if (machine.kind !== "answered") continue;
        for (const agent of ["claude", "codex"] as const) {
          for (const home of homesOf(machine.reply.agents, agent).filter((home) => usableHome(home) && listed(home))) {
            known.push({
              login: { machine: machine.reply.machine, agent, home: homeChoice(home) },
              label: this.loginLabel(agent, machine.reply.machine, home),
              models: home.models.kind === "known" ? home.models.value : [],
            });
          }
        }
      }
      this.known = known;
    } catch {
      this.known = [];
    }
    this.draw();
  }

  private loginLabel(agent: "codex" | "claude", machine: string, facts: AgentFacts): string {
    const who = accountOf(facts).email ?? homeLabel(facts, this.options.homeWords);
    return `${agentName(agent)} ${machine} ${who}`;
  }

  private dirty(): boolean {
    return !!this.draft && !!this.view && JSON.stringify(this.draft) !== JSON.stringify(this.view.collab);
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLElement>("[data-act]");
    if (!button || (button as HTMLButtonElement).disabled) return;
    const draft = this.draft;
    switch (button.dataset.act) {
      case "replay":
        return;
      case "retry":
        void this.load();
        return;
      case "turn":
        if (!draft) return;
        draft.on = true;
        void this.save().then(() => {
          if (this.view?.collab.on && !this.problem) this.celebrate(this.options.words.collaboration.name, this.options.words.collaboration.after, (shape) => collaboration(this.options.words.collaboration.film, shape));
        });
        return;
      case "turn-off":
        this.askOff(this.options.words.collaboration.name, this.options.words.collaboration.offWhat, () => this.turnOff(), () => this.draw());
        return;
      case "caller": {
        if (!draft) return;
        const login = this.known[Number(button.dataset.index)]?.login;
        if (!login) return;
        draft.callers = draft.callers.some((one) => sameAgentLogin(one, login)) ? draft.callers.filter((one) => !sameAgentLogin(one, login)) : [...draft.callers, login];
        this.draw();
        return;
      }
      case "add-callee": {
        if (!draft) return;
        const codex = this.known.find((one) => one.login.agent === "codex");
        if (!codex) return;
        draft.callees.push({ name: this.freeName(), use_when: "", login: codex.login, model: null, effort: null, screen: true });
        this.draw();
        return;
      }
      case "remove-callee":
        if (!draft) return;
        draft.callees.splice(Number(button.dataset.index), 1);
        this.draw();
        return;
      case "reset-prompt":
        if (!draft) return;
        draft.prompt = null;
        this.draw();
        return;
      case "save":
        void this.save();
        return;
    }
  }

  private freeName(): string {
    const taken = new Set(this.draft?.callees.map((callee) => callee.name));
    for (let n = 1; ; n += 1) if (!taken.has(n === 1 ? "gpt" : `gpt-${n}`)) return n === 1 ? "gpt" : `gpt-${n}`;
  }

  /** Text typed and choices made change the draft only; the page is drawn
   *  again for what depends on them (a login's models, the save button). */
  private input(event: Event): void {
    const field = event.target as HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;
    const draft = this.draft;
    if (!draft || !field.dataset.field) return;
    const callee = field.dataset.index !== undefined ? draft.callees[Number(field.dataset.index)] : undefined;
    switch (field.dataset.field) {
      case "name":
        if (callee) callee.name = field.value.toLowerCase().replace(/[^a-z0-9-]/g, "");
        break;
      case "use":
        if (callee) callee.use_when = field.value;
        break;
      case "login":
        if (callee) {
          const known = this.known[Number(field.value)];
          if (known) callee.login = known.login;
          callee.model = null;
          callee.effort = null;
          this.draw();
        }
        break;
      case "model":
        if (callee) {
          callee.model = field.value || null;
          this.draw();
        }
        break;
      case "screen":
        if (callee) callee.screen = (field as HTMLInputElement).checked;
        break;
      case "prompt":
        draft.prompt = field.value === this.view?.default_prompt ? null : field.value;
        break;
      case "rules":
        draft.rules = field.value;
        break;
    }
    this.saved = false;
    this.drawSave();
  }

  /** Saves for the account, or in no network as this computer's own. What
   *  failed is said where the user acted, after the page reads the
   *  settings again. */
  private async save(): Promise<void> {
    const account = this.account;
    const draft = this.draft;
    if (!draft || !this.view) return;
    const button = this.body.querySelector<HTMLButtonElement>(".feature-save");
    if (button) button.disabled = true;
    try {
      const version = await writeCollab(account?.hubName ?? null, account?.account ?? null, draft);
      draft.version = version;
      this.view = { ...this.view, collab: structuredClone(draft) };
      this.problem = null;
      this.saved = true;
    } catch (error) {
      const problem = { text: this.options.words.failed(detailOf(error)), detail: detailOf(error) };
      await this.load();
      if (this.view) {
        this.problem = problem;
        this.draw();
      }
      return;
    }
    this.draw();
  }

  /** Turns model collaboration off for the account, or in no network for
   *  this computer, with what else the draft holds; fails as the World says,
   *  for the question to show. */
  private async turnOff(): Promise<void> {
    const account = this.account;
    if (!this.draft || !this.view) return;
    const draft = { ...structuredClone(this.draft), on: false };
    draft.version = await writeCollab(account?.hubName ?? null, account?.account ?? null, draft);
    this.view = { ...this.view, collab: structuredClone(draft) };
    this.draft = draft;
    this.problem = null;
    this.saved = false;
  }

  /** Watches the page's film again, over the page. */
  private replayFilm: (() => void) | null = null;

  private replay(): void {
    this.replayFilm?.();
  }

  /** What model collaboration gives the user: a ready text that teaches
   *  the calling model to hand a part over in plain words, and nothing to
   *  operate by hand. */
  private gives(): string {
    return `<ul class="feature-gives">${this.options.words.collaboration.gives
      .map(([mark, line]) => `<li><span class="feature-give-mark">${glyph(mark, 16)}</span><span>${escape(line)}</span></li>`)
      .join("")}</ul>`;
  }

  private draw(): void {
    const { words } = this.options;
    const collab = this.draft;
    const on = !!collab?.on;
    const gives = this.gives();
    const turn = !collab
        ? this.problem
          ? `<p class="feature-note feature-trouble" title="${escape(this.problem.detail)}">${escape(this.problem.text)}<button class="text-button quiet-button" type="button" data-act="retry">${glyph("regenerate", 14)}<span>${escape(words.retry)}</span></button></p>`
          : `<p class="feature-note">${escape(words.reading)}</p>`
        : on
          ? ""
          : `<button class="sheet-go feature-turn" type="button" data-act="turn" data-on="1">${escape(words.turnOn)}</button>${
              this.problem ? `<p class="feature-note feature-trouble" title="${escape(this.problem.detail)}">${escape(this.problem.text)}</p>` : ""}`;
    const top = on ? this.onTop(words.collaboration.after) : `<div class="feature-intro with-gives"><div class="feature-film"></div>${gives}${turn}</div>`;
    // The film keeps playing while the page around it is drawn again.
    const playing = !on ? this.body.querySelector(".feature-intro .film") : null;
    if (!playing) this.dropFilms();
    this.body.classList.toggle("features-film", !on);
    this.body.classList.toggle("features-on", on);
    this.body.innerHTML = top + (on && collab ? this.settings(collab) + this.offWay(words.collaboration.name) : "");
    const make = (shape: Shape) => collaboration(words.collaboration.film, shape);
    if (playing) this.body.querySelector(".feature-film")?.replaceChildren(playing);
    else this.film(this.body.querySelector(on ? ".feature-still" : ".feature-film"), make, words.collaboration.name, on);
    this.mountSliders();
    this.drawNow();
    this.drawSave();
  }

  private settings(collab: Collab): string {
    const { words } = this.options;
    const callers = this.known.length
      ? this.known
          .map((known, index) => {
            const chosen = collab.callers.some((one) => sameAgentLogin(one, known.login));
            return `<button class="feature-chip${chosen ? " chosen" : ""}" type="button" data-act="caller" data-index="${index}" aria-pressed="${chosen}">${glyph(known.login.agent, 14)}<span>${escape(known.label)}</span></button>`;
          })
          .join("")
      : `<p class="feature-quiet">${escape(words.noLogins)}</p>`;
    const codex = this.known.map((known, index) => ({ known, index })).filter(({ known }) => known.login.agent === "codex");
    const callees = collab.callees
      .map((callee, index) => {
        const at = codex.find(({ known }) => sameAgentLogin(known.login, callee.login));
        const models = at?.known.models ?? [];
        return `<li class="callee-card">
          <div class="callee-row"><label class="callee-field callee-name"><span>${escape(words.name)}</span><input type="text" value="${escape(callee.name)}" data-field="name" data-index="${index}" pattern="[a-z0-9-]+" title="${escape(words.namePattern)}" spellcheck="false" autocomplete="off" autocapitalize="off"></label>
            <button class="icon-button callee-remove" type="button" data-act="remove-callee" data-index="${index}" title="${escape(words.removeCallee)}" aria-label="${escape(words.removeCallee)}">${glyph("close", 16)}</button></div>
          <label class="callee-field"><span>${escape(words.useWhen)}</span><input type="text" value="${escape(callee.use_when)}" data-field="use" data-index="${index}"></label>
          <label class="callee-field"><span>${escape(words.login)}</span><select data-field="login" data-index="${index}">${codex
            .map(({ known, index: i }) => `<option value="${i}"${at?.index === i ? " selected" : ""}>${escape(known.label)}</option>`)
            .join("")}${at ? "" : `<option selected disabled>${escape(`${callee.login.machine} ${callee.login.home ?? ""}`)}</option>`}</select></label>
          <label class="callee-field"><span>${escape(words.model)}</span><select data-field="model" data-index="${index}"><option value=""${callee.model ? "" : " selected"}>${escape(words.modelDefault)}</option>${models
            .map((model) => `<option value="${escape(model.model)}"${callee.model === model.model ? " selected" : ""}>${escape(model.display_name)}</option>`)
            .join("")}</select></label>
          <div class="callee-field"><span>${escape(words.effort)}</span><div class="callee-effort" data-index="${index}"></div></div>
          <label class="callee-switch" title="${escape(words.screenSaid)}"><input type="checkbox" data-field="screen" data-index="${index}"${callee.screen ? " checked" : ""}><span>${escape(words.screen)}</span></label>
        </li>`;
      })
      .join("");
    const prompt = collab.prompt ?? this.view?.default_prompt ?? "";
    return `<section class="feature-section"><h2 class="feature-heading" title="${escape(words.callersSaid)}">${escape(words.callers)}</h2><div class="feature-chips">${callers}</div></section>
      <section class="feature-section"><h2 class="feature-heading" title="${escape(words.openCodex)}">${escape(words.callees)}</h2>
        <ul class="callee-cards">${callees}<li><button class="callee-add" type="button" data-act="add-callee"${codex.length ? "" : ` disabled title="${escape(words.noCodex)}"`}>${glyph("plus", 16)}<span>${escape(words.addCallee)}</span></button></li></ul></section>
      <section class="feature-section"><h2 class="feature-heading">${escape(words.prompt)}${collab.prompt !== null ? `<button class="text-button quiet-button" type="button" data-act="reset-prompt">${escape(words.promptReset)}</button>` : ""}</h2>
        <textarea class="feature-text" data-field="prompt" rows="8" spellcheck="false">${escape(prompt)}</textarea></section>
      <section class="feature-section"><h2 class="feature-heading">${escape(words.rules)}</h2>
        <textarea class="feature-text" data-field="rules" rows="4" placeholder="${escape(words.rulesHint)}">${escape(collab.rules)}</textarea></section>
      <section class="feature-section"><h2 class="feature-heading">${escape(words.now)}</h2><ul class="feature-now"></ul></section>
      <div class="feature-save-bar"><span class="feature-problem"></span><button class="sheet-go feature-save" type="button" data-act="save">${escape(words.save)}</button></div>`;
  }

  /** Each callee's effort, as the model chosen for it offers. */
  private mountSliders(): void {
    const { words, english } = this.options;
    this.sliders = [];
    for (const holder of this.body.querySelectorAll<HTMLElement>(".callee-effort")) {
      const index = Number(holder.dataset.index);
      const callee = this.draft?.callees[index];
      if (!callee) continue;
      const models = this.known.find((one) => sameAgentLogin(one.login, callee.login))?.models ?? [];
      const model = models.find((one) => one.model === callee.model) ?? models.find((one) => one.is_default) ?? models[0];
      const slider = new EffortSlider(words.effort, (value) => {
        callee.effort = value;
        this.saved = false;
        this.drawSave();
      });
      slider.set((model?.reasoning_efforts ?? []).map((one) => ({ value: one.effort, label: effortLabel(one.effort, english) })), callee.effort ?? model?.default_reasoning_effort ?? null);
      holder.append(slider.element);
      this.sliders.push(slider);
    }
  }

  /** Who works for whom now: the GPTs asked in that are at work. */
  private drawNow(): void {
    const list = this.body.querySelector<HTMLElement>(".feature-now");
    if (!list) return;
    const { words } = this.options;
    const residents = residentsOf(this.options.store.world.peek());
    const working = residents.filter((r) => r.about.origin.kind === "colleague" && r.now?.activity === "busy");
    const html = working.length
      ? working
          .map((r) => {
            const origin = r.about.origin.kind === "colleague" ? r.about.origin : null;
            const asker = origin ? residents.find((one) => one.about.id === origin.asked_by.resident && one.about.machine === origin.asked_by.machine) : undefined;
            const askerName = asker ? (titleOf(asker) ?? `${agentName(asker.about.host.agent)} ${asker.about.machine}`) : (origin?.asked_by.resident ?? "");
            return `<li><a href="${link({ kind: "talk", address: { resident: r.about.id, machine: r.about.machine } })}">${glyph("codex", 14)}<span>${escape(titleOf(r) ?? r.about.id)}</span><span class="feature-now-for">${escape(words.workingFor(askerName))} ${escape(r.about.machine)}</span></a></li>`;
          })
          .join("")
      : `<li class="feature-quiet">${escape(words.nobodyNow)}</li>`;
    if (list.innerHTML !== html) list.innerHTML = html;
  }

  private drawSave(): void {
    const bar = this.body.querySelector<HTMLElement>(".feature-save-bar");
    if (!bar) return;
    const { words } = this.options;
    const dirty = this.dirty();
    bar.classList.toggle("shown", dirty || this.saved || !!this.problem);
    const button = bar.querySelector<HTMLButtonElement>(".feature-save")!;
    button.disabled = !dirty;
    button.innerHTML = !dirty && this.saved ? `${glyph("check", 14)}<span>${escape(words.saved)}</span>` : escape(words.save);
    const problem = bar.querySelector<HTMLElement>(".feature-problem")!;
    problem.textContent = this.problem?.text ?? "";
    problem.title = this.problem?.detail ?? "";
  }
}
