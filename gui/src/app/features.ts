// The features (`world-gui.md`, 功能): a card for each, and each its own
// page. A feature is off until turned on; off, its page is the drawing that
// tells what it does and the button that turns it on; on, the drawing waits
// folded at the top, to be watched again, and the settings are below.
//
// 「对话」 is the first: talking with the agents in the interface, wherever
// they are; it is kept per device and has no settings. Model collaboration:
// for the whole account, which agents may hand a part of their work to a
// GPT (callers), which GPTs they may hand it to (callees: a Codex login
// each, its model and effort, the name and the line the calling model
// chooses by, whether it may use the screen), the text the callers are given
// and the rules every callee is given. Project memory is in development: its
// card says so and its page says nothing else.

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
import { link, type Feature } from "./places";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import { residentsOf, type Store } from "./store";
import { deviceFeatures } from "./device-features";
import { storyboard } from "./storyboard";

export interface FeaturesOptions {
  store: Store;
  words: FeaturesWords;
  homeWords: HomeWords;
  english: boolean;
  head(title: string, actions?: string): string;
  feature: Feature | null;
  agents(): Promise<WorldAgents>;
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

  constructor(private readonly options: FeaturesOptions) {
    const { words, feature } = options;
    this.title = feature === "collaboration" ? words.collaboration.name : feature === "memory" ? words.memory.name : feature === "conversations" ? words.conversations.name : words.title;
    this.element = document.createElement("div");
    this.element.className = "features-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`)}<div class="features-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".features-body")!;
    this.body.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-act='replay']")) this.replay();
    });
    if (feature === null) this.cards();
    else if (feature === "conversations") this.conversations();
    else if (feature === "memory") this.body.innerHTML = `<p class="feature-coming">${glyph("info", 18)}<span>${escape(words.coming)}</span></p>`;
    else this.collaboration();
  }

  leave(): void {
    this.stop();
    this.sliders = [];
  }

  /** Every feature, a card each. */
  private cards(): void {
    const { words } = this.options;
    const tag = (on: boolean | null) => (on === null ? "" : `<span class="tag${on ? " tag-own" : ""}">${escape(on ? words.on : words.off)}</span>`);
    const draw = (on: boolean | null) => {
      const state = tag(on);
      this.body.innerHTML = `<ul class="feature-cards">
        <li><a class="feature-card" href="${link({ kind: "feature", feature: "conversations" })}">
          <span class="feature-card-art">${glyph("phone", 22)}${glyph("desktop", 22)}</span>
          <span class="feature-card-name">${escape(words.conversations.name)}</span>
          <span class="feature-card-line">${escape(words.conversations.line)}</span>${tag(deviceFeatures.conversations.peek())}</a></li>
        <li><a class="feature-card" href="${link({ kind: "feature", feature: "collaboration" })}">
          <span class="feature-card-art">${glyph("codex", 22)}${glyph("claude", 22)}</span>
          <span class="feature-card-name">${escape(words.collaboration.name)}</span>
          <span class="feature-card-line">${escape(words.collaboration.line)}</span>${state}</a></li>
        <li><a class="feature-card coming" href="${link({ kind: "feature", feature: "memory" })}">
          <span class="feature-card-art">${glyph("folder", 22)}</span>
          <span class="feature-card-name">${escape(words.memory.name)}</span>
          <span class="feature-card-line">${escape(words.memory.line)}</span><span class="tag">${escape(words.coming)}</span></a></li>
      </ul>`;
    };
    draw(null);
    // In no network, this computer keeps its own settings.
    const [first] = loginsOf(this.options.store.world.peek()?.accounts);
    readCollab(first?.hub ?? null, first?.account ?? null)
      .then((view) => draw(view.collab.on))
      .catch(() => draw(false));
  }

  /** 「对话」: its drawing and its switch; on, the drawing folds to the top. */
  private conversations(): void {
    const { words } = this.options;
    const draw = () => {
      const on = deviceFeatures.conversations.peek();
      const board = storyboard("conversations", words.conversations.frames, words.conversations.name);
      this.body.innerHTML = on
        ? `<div class="feature-top"><div class="storyboard-strip">${board.replace('class="storyboard"', 'class="storyboard folded"')}</div>
            <div class="feature-top-acts"><button class="text-button quiet-button" type="button" data-act="replay">${glyph("regenerate", 14)}<span>${escape(words.replay)}</span></button>
            <button class="text-button quiet-button" type="button" data-turn="0">${escape(words.turnOff)}</button></div></div>
            <p class="feature-quiet feature-after">${escape(words.conversations.after)}</p>`
        : `<div class="feature-intro">${board}<button class="sheet-go feature-turn" type="button" data-turn="1">${escape(words.turnOn)}</button></div>`;
      const intro = this.body.querySelector<HTMLElement>(".feature-intro .storyboard");
      if (intro) drawAsSeen(intro);
    };
    draw();
    this.body.addEventListener("click", (event) => {
      const turn = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-turn]");
      if (!turn || turn.disabled) return;
      turn.disabled = true;
      void deviceFeatures.setConversations(turn.dataset.turn === "1").then(draw, (error: unknown) => {
        turn.disabled = false;
        turn.title = detailOf(error);
      });
    });
  }

  private collaboration(): void {
    const { store } = this.options;
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
      this.view = await readCollab(account?.hub ?? null, account?.account ?? null);
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
    return `${agentName(agent)} · ${machine} · ${who}`;
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
        draft.on = button.dataset.on === "1";
        void this.save();
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

  private async save(): Promise<void> {
    const account = this.account;
    const draft = this.draft;
    if (!account || !draft || !this.view) return;
    const button = this.body.querySelector<HTMLButtonElement>(".feature-save");
    if (button) button.disabled = true;
    try {
      const version = await writeCollab(account.hub, account.account, draft);
      draft.version = version;
      this.view = { ...this.view, collab: structuredClone(draft) };
      this.problem = null;
      this.saved = true;
    } catch (error) {
      this.problem = { text: this.options.words.failed(detailOf(error)), detail: detailOf(error) };
      await this.load();
      return;
    }
    this.draw();
  }

  private replay(): void {
    const board = this.body.querySelector<HTMLElement>(".storyboard");
    if (!board) return;
    board.classList.remove("folded");
    const fresh = board.cloneNode(true) as HTMLElement;
    board.replaceWith(fresh);
  }

  private draw(): void {
    const { words } = this.options;
    const collab = this.draft;
    const on = !!collab?.on;
    const board = storyboard("collaboration", words.collaboration.frames, words.collaboration.name);
    // What it gives the user: a ready text that teaches the calling model to
    // hand a part over in plain words, and nothing to operate by hand.
    const gives = `<ul class="feature-gives">${words.collaboration.gives
      .map(([mark, line]) => `<li><span class="feature-give-mark">${glyph(mark, 16)}</span><span>${escape(line)}</span></li>`)
      .join("")}</ul>`;
    const turn = !collab
        ? this.problem
          ? `<p class="feature-note feature-trouble" title="${escape(this.problem.detail)}">${escape(this.problem.text)}<button class="text-button quiet-button" type="button" data-act="retry">${glyph("regenerate", 14)}<span>${escape(words.retry)}</span></button></p>`
          : `<p class="feature-note">${escape(words.reading)}</p>`
        : on
          ? ""
          : `<button class="sheet-go feature-turn" type="button" data-act="turn" data-on="1">${escape(words.turnOn)}</button>`;
    const top = on
      ? `<div class="feature-top"><div class="storyboard-strip">${board.replace('class="storyboard"', 'class="storyboard folded"')}</div>
          <div class="feature-top-acts"><button class="text-button quiet-button" type="button" data-act="replay">${glyph("regenerate", 14)}<span>${escape(words.replay)}</span></button>
          <button class="text-button quiet-button" type="button" data-act="turn" data-on="0">${escape(words.turnOff)}</button></div></div>`
      : `<div class="feature-intro">${board}${gives}${turn}</div>`;
    this.body.innerHTML = top + (on && collab ? this.settings(collab) : "");
    const intro = this.body.querySelector<HTMLElement>(".feature-intro .storyboard");
    if (intro) drawAsSeen(intro);
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
            .join("")}${at ? "" : `<option selected disabled>${escape(`${callee.login.machine} · ${callee.login.home ?? ""}`)}</option>`}</select></label>
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
            const askerName = asker ? (titleOf(asker) ?? `${agentName(asker.about.host.agent)} · ${asker.about.machine}`) : (origin?.asked_by.resident ?? "");
            return `<li><a href="${link({ kind: "talk", address: { resident: r.about.id, machine: r.about.machine } })}">${glyph("codex", 14)}<span>${escape(titleOf(r) ?? r.about.id)}</span><span class="feature-now-for">${escape(words.workingFor(askerName))} · ${escape(r.about.machine)}</span></a></li>`;
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

/** Where the frames are a strip swiped one by one (a phone), each is drawn
 *  when it comes into view rather than all at once off screen: its strokes
 *  keep their order and pace, counted from its own first stroke. */
function drawAsSeen(board: HTMLElement): void {
  if (board.scrollWidth <= board.clientWidth + 1) return;
  const frames = [...board.querySelectorAll<HTMLElement>(".sb-frame")];
  for (const frame of frames) {
    const timed = [...frame.querySelectorAll<HTMLElement | SVGElement>("[style*='animation-delay']")];
    const delay = (element: HTMLElement | SVGElement) => parseFloat(element.style.animationDelay) || 0;
    const first = Math.min(...timed.map(delay));
    for (const element of timed) element.style.animationDelay = `${delay(element) - first}ms`;
    frame.classList.add("waiting");
  }
  const seen = new IntersectionObserver(
    (entries) => {
      for (const entry of entries.filter((entry) => entry.isIntersecting)) {
        entry.target.classList.remove("waiting");
        seen.unobserve(entry.target);
      }
    },
    { root: board, threshold: 0.6 },
  );
  for (const frame of frames) seen.observe(frame);
  // It plays through by itself: each frame drawn, a moment to read it, the
  // next slides in; a hand on the strip takes over from then on.
  let held = false;
  board.addEventListener("pointerdown", () => (held = true), { once: true });
  for (const [index, frame] of frames.entries()) {
    const next = frames[index + 1];
    if (!next) break;
    frame.querySelector("figcaption")?.addEventListener(
      "animationend",
      () =>
        setTimeout(() => {
          if (!held && board.isConnected) next.scrollIntoView({ behavior: "smooth", inline: "center", block: "nearest" });
        }, READ_MS),
      { once: true },
    );
  }
}

/** How long a drawn frame stays before the next slides in. */
const READ_MS = 1400;
