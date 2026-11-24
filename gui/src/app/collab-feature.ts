// Model collaboration's tab of the conversations' settings
// (`world-colleagues.md`, `world-gui.md` 功能). Off, it is the film, what
// it gives and the way to turn it on. On, under a still of the film: the
// presets, one row each with its switch, a row opened to rewrite it
// (`./preset-sheet`); the callers in one line, chosen on a sheet of their
// own (`./callers-sheet`); the rules every preset is given, on writing
// paper; who works for whom now; and, last and light, the way to turn it
// off. Each change is written as it is made, over the version that stands.

import { effortLabel } from "../conversation/effort-slider";
import { agentGlyph, glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { modelName } from "../conversation/view";
import { collaboration } from "../film/collaboration";
import type { Built, Shape } from "../film/reel";
import type { Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentModel, WorldAgents } from "../world/agents";
import { kindOf, readCollab, sameAgentLogin, writeCollab, type Collab, type CollabView } from "../world/collab";
import { agentName, titleOf } from "../world/names";
import { accountOf, homeChoice, homeLabel, homesOf, listed, usableHome, type HomeWords } from "./agent-homes";
import { CallersSheet, type CallerLogin } from "./callers-sheet";
import { atOnceChoices, atOnceInAll, atOnceSaid, defaultLogin, fresh, shownPresets, withRemoved, withSaved, type CalleeLogin, type Shown } from "./collab-presets";
import type { Confirm } from "./confirm";
import type { FeatureFrame } from "./history-feature";
import { link } from "./places";
import { PresetSheet } from "./preset-sheet";
import { detailOf } from "./problem";
import { inNoNetwork } from "./reach";
import { effect } from "./signals";
import { residentsOf, type Store } from "./store";
import { waiting } from "./waiting";

export interface CollabOptions {
  store: Store;
  english: boolean;
  homeWords: HomeWords;
  agents(): Promise<WorldAgents>;
  /** Asks before a preset is removed. */
  confirm: Confirm;
}

/** How many times a change is made over the version standing now before it
 *  is given up. */
const SAVE_TRIES = 3;
const SAID_MS = 2400;

/** A login as the page knows it: whose it is, and the models it offers. */
type Known = CallerLogin & { models: AgentModel[] };

export class CollabFeature {
  /** Ends what follows the World. */
  readonly stop: () => void;
  private readonly make: (shape: Shape) => Built;
  private readonly presetSheet: PresetSheet;
  private readonly callersSheet: CallersSheet;
  private account: Login | null = null;
  /** Whether this computer is in no network: it asks no colleagues, and
   *  its page offers the ways into one instead of settings. */
  private alone = false;
  /** Whether the World was seen once, so that where the settings are kept
   *  is known. */
  private placed = false;
  private view: CollabView | null = null;
  private problem: { text: string; detail: string } | null = null;
  private known: Known[] = [];
  /** The presets as their rows were last drawn. */
  private rows: Shown[] = [];
  private busy = false;
  /** The rules as typed and not saved. */
  private typed: string | null = null;
  private said = false;
  private saidTimer = 0;
  /** Done once the machines have told of their logins, or failed to. */
  private readonly asked: Promise<void>;

  constructor(
    private readonly frame: FeatureFrame,
    private readonly options: CollabOptions,
  ) {
    const { words } = frame;
    this.make = (shape) => collaboration(words.collaboration.film, shape);
    this.presetSheet = new PresetSheet(words, options.english);
    this.callersSheet = new CallersSheet(words);
    this.place();
    this.draw();
    this.asked = this.ask();
    const unfollow = effect(() => {
      options.store.world.get();
      if (this.place()) this.draw();
      this.drawNow();
    });
    this.stop = () => {
      unfollow();
      clearTimeout(this.saidTimer);
    };
    frame.body.addEventListener("click", (event) => this.click(event));
    frame.body.addEventListener("input", (event) => {
      const field = event.target as HTMLTextAreaElement;
      if (!field.matches(".cp-rules")) return;
      this.typed = field.value === this.view?.collab.rules ? null : field.value;
      this.drawFoot();
    });
    frame.body.addEventListener("keydown", (event) => {
      if ((event.target as HTMLElement).matches(".cp-rules") && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void this.saveRules();
      }
    });
  }

  /** The film over the page, as "watch again" plays it. */
  get film(): (shape: Shape) => Built {
    return this.make;
  }

  /** Where the settings are kept, as the World now says: the first
   *  account's, read again when that changes; none in no network. Whether
   *  it changed. */
  private place(): boolean {
    const world = this.options.store.world.peek();
    if (!world) return false;
    const alone = inNoNetwork(world);
    const account = alone ? null : (loginsOf(world.accounts)[0] ?? null);
    if (this.placed && alone === this.alone && account?.hub === this.account?.hub && account?.account === this.account?.account) return false;
    this.placed = true;
    this.alone = alone;
    this.account = account;
    this.view = null;
    this.problem = null;
    this.typed = null;
    if (!alone && account) void this.load();
    return true;
  }

  /** The account's settings. */
  private async load(): Promise<void> {
    const account = this.account;
    let view: CollabView | null = null;
    let problem: { text: string; detail: string } | null = null;
    try {
      view = await readCollab(account?.hubName ?? null, account?.account ?? null);
    } catch (error) {
      problem = { text: this.frame.words.unavailable, detail: detailOf(error) };
    }
    // Read for an account no longer the one shown: the newer read draws.
    if (account !== this.account) return;
    this.view = view;
    this.problem = problem;
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
              who: accountOf(home).email ?? homeLabel(home, this.options.homeWords),
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

  /** The logins a preset may run in: Codex's first, then Claude Code's;
   *  Codex's alone where presets are not kept, as a hub from before them
   *  keeps a callee in a Codex login alone. */
  private logins(): CalleeLogin[] {
    const order = (agent: string) => (agent === "codex" ? 0 : 1);
    const legacy = this.legacy();
    return [...this.known]
      .filter((one) => !legacy || one.login.agent === "codex")
      .sort((a, b) => order(a.login.agent) - order(b.login.agent))
      .map((one) => ({ login: one.login, label: `${agentName(one.login.agent)} ${one.login.machine} ${one.who}`, models: one.models }));
  }

  /** Whether presets are not kept where the settings are: its callees stay
   *  as they were before presets. */
  private legacy(): boolean {
    return !this.view?.keeps_presets;
  }

  /** Makes `change` over the settings that stand and shows what it left.
   *  Written elsewhere since they were read, they are read again and the
   *  change made over the version standing now; what the World refuses is
   *  the caller's to say. */
  private async commit(change: (collab: Collab) => Collab): Promise<void> {
    const account = this.account;
    this.busy = true;
    try {
      for (let tries = 1; ; tries += 1) {
        const view = this.view;
        if (!view) throw new Error(this.frame.words.unavailable);
        const next = change(structuredClone(view.collab));
        try {
          const version = await writeCollab(account?.hubName ?? null, account?.account ?? null, next);
          if (account === this.account) this.view = { ...view, collab: { ...next, version } };
          this.problem = null;
          return;
        } catch (error) {
          const elsewhere = /model collaboration changed/.test(detailOf(error));
          await this.reread();
          if (!elsewhere || tries >= SAVE_TRIES) throw error;
        }
      }
    } finally {
      this.busy = false;
      this.draw();
    }
  }

  /** The version that stands now, after a change did not go through. */
  private async reread(): Promise<void> {
    const account = this.account;
    try {
      const view = await readCollab(account?.hubName ?? null, account?.account ?? null);
      if (account === this.account) this.view = view;
    } catch {
      // What was shown stays.
    }
  }

  /** A change made from the page itself: what the World refuses is said on
   *  the page. */
  private change(change: (collab: Collab) => Collab): Promise<void> {
    return this.commit(change).catch((error: unknown) => {
      this.problem = { text: this.frame.words.failed(detailOf(error)), detail: detailOf(error) };
      this.draw();
    });
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLElement>("[data-act]");
    if (!button || (button as HTMLButtonElement).disabled || this.busy) return;
    const { words } = this.frame;
    const view = this.view;
    switch (button.dataset.act) {
      case "retry":
        void this.load();
        return;
      case "turn":
        // The reviewer is what an account starts with: turned on with
        // collaboration, where it keeps no preset yet and has a login for
        // it, which its machines are heard out for.
        this.busy = true;
        this.draw();
        void this.asked
          .then(() =>
            this.commit((collab) => {
              const on = { ...collab, on: true };
              // As it always was, the reviewer an account starts with is GPT.
              const login = defaultLogin(this.logins());
              const none = !collab.callees.length && !collab.shelved?.length;
              return none && login?.login.agent === "codex" && !this.legacy() ? withSaved(on, null, fresh(collab, "review", login), true, false) : on;
            }),
          )
          .then(
            () => this.frame.celebrate(words.collaboration.name, words.collaboration.after, this.make),
            (error: unknown) => {
              this.problem = { text: words.failed(detailOf(error)), detail: detailOf(error) };
              this.draw();
            },
          );
        return;
      case "turn-off":
        this.frame.askOff(words.collaboration.name, words.collaboration.offWhat, () => this.commit((collab) => ({ ...collab, on: false })), () => this.draw());
        return;
      case "toggle": {
        const shown = this.rows[Number(button.dataset.at)];
        if (!shown) return;
        // One with nowhere to run yet is opened, to be given a login.
        if (!shown.on && !shown.callee.login.machine) this.open(shown);
        else void this.change((collab) => withSaved(collab, shown.offered ? null : shown.callee.name, shown.callee, !shown.on, false));
        return;
      }
      case "open":
        this.open(this.rows[Number(button.dataset.at)] ?? null);
        return;
      case "in-all": {
        const at_once = atOnceSaid(button.dataset.value ?? "");
        void this.change((collab) => ({ ...collab, at_once }));
        return;
      }
      case "add":
        this.open(null);
        return;
      case "callers":
        if (!view) return;
        this.callersSheet.open({
          known: this.known,
          chosen: view.collab.callers,
          save: (callers) => this.commit((collab) => ({ ...collab, callers })),
        });
        return;
      case "save-rules":
        void this.saveRules();
        return;
    }
  }

  /** Opens a preset to rewrite it, or a new one. */
  private open(shown: Shown | null): void {
    const view = this.view;
    if (!view) return;
    const { words, confirm } = { words: this.frame.words, confirm: this.options.confirm };
    const logins = this.logins();
    const legacy = this.legacy();
    const kept = shown && !shown.offered ? shown.callee.name : null;
    this.presetSheet.open({
      shown,
      taken: [...view.collab.callees, ...(view.collab.shelved ?? [])].map((callee) => callee.name).filter((name) => name !== kept),
      logins,
      texts: view.presets ?? [],
      legacy,
      fresh: (kind) => fresh(view.collab, kind, defaultLogin(logins)),
      // A new one is turned on; one rewritten stays as its switch stands.
      save: (callee, previous) => this.commit((collab) => withSaved(collab, previous, callee, shown ? shown.on : true, legacy)),
      remove: () => {
        if (!kept) return;
        confirm.ask({
          title: words.presets.removeTitle(kept),
          what: words.presets.removeWhat,
          go: words.removeCallee,
          act: () => this.commit((collab) => withRemoved(collab, kept, legacy)),
          done: () => {},
        });
      },
    });
  }

  private async saveRules(): Promise<void> {
    const rules = this.typed;
    if (rules === null || this.busy) return;
    try {
      await this.commit((collab) => ({ ...collab, rules }));
      this.typed = null;
      this.said = true;
      clearTimeout(this.saidTimer);
      this.saidTimer = window.setTimeout(() => {
        this.said = false;
        this.drawFoot();
      }, SAID_MS);
    } catch (error) {
      this.problem = { text: this.frame.words.failed(detailOf(error)), detail: detailOf(error) };
    }
    this.draw();
  }

  /** In no network, where the switch would be: that it needs one, and the
   *  ways into one, as the machines' page offers them. */
  private ways(): string {
    const { words } = this.frame;
    return `<div class="feature-ways"><p class="feature-note">${escape(words.alone)}</p><div class="feature-way-links"><a class="sheet-go" href="${link({ kind: "login", way: "new" })}">${escape(words.newNetwork)}</a><a class="text-button quiet-button" href="${link({ kind: "login", way: "join" })}">${escape(words.joinNetwork)}</a></div></div>`;
  }

  private draw(): void {
    const { body, words } = this.frame;
    const collab = this.view?.collab ?? null;
    const on = !!collab?.on;
    const trouble = this.problem ? `<p class="feature-note feature-trouble" title="${escape(this.problem.detail)}">${escape(this.problem.text)}</p>` : "";
    const turn = this.alone
      ? this.ways()
      : this.placed && !this.account
        ? `<p class="feature-note">${escape(words.noAccount)}</p>`
        : !collab
          ? this.problem
            ? `<p class="feature-note feature-trouble" title="${escape(this.problem.detail)}">${escape(this.problem.text)}<button class="text-button quiet-button" type="button" data-act="retry">${glyph("regenerate", 14)}<span>${escape(words.retry)}</span></button></p>`
            : waiting({ label: words.reading, shape: "lines", rows: 3 })
          : on
            ? ""
            : `<button class="sheet-go feature-turn" type="button" data-act="turn" data-on="1"${this.busy ? " disabled" : ""}>${escape(words.turnOn)}</button>${trouble}`;
    const top = on ? this.frame.onTop(words.collaboration.after) : `<div class="feature-intro with-gives"><div class="feature-film"></div>${this.frame.gives(words.collaboration.gives)}${turn}</div>`;
    // The film keeps playing while the page around it is drawn again.
    const playing = !on ? body.querySelector(".feature-intro .film") : null;
    if (!playing) this.frame.dropFilms();
    body.classList.toggle("features-film", !on);
    body.classList.toggle("features-on", on);
    body.innerHTML = top + (on && this.view ? this.settings(this.view, trouble) + this.frame.offWay(words.collaboration.name) : "");
    if (playing) body.querySelector(".feature-film")?.replaceChildren(playing);
    else this.frame.film(body.querySelector(on ? ".feature-still" : ".feature-film"), this.make, words.collaboration.name, on);
    this.drawNow();
    this.drawFoot();
  }

  private settings(view: CollabView, trouble: string): string {
    const { words } = this.frame;
    const p = words.presets;
    const { collab } = view;
    const legacy = this.legacy();
    const logins = this.logins();
    this.rows = shownPresets(view, defaultLogin(logins));
    const rows = this.rows.map((shown, at) => this.row(shown, at, legacy)).join("");
    const add = `<button class="text-button quiet-button cp-act" type="button" data-act="add"${logins.length ? "" : ` disabled title="${escape(words.noLogins)}"`}>${glyph("plus", 15)}<span>${escape(p.add)}</span></button>`;
    const pick = `<button class="text-button quiet-button cp-act" type="button" data-act="callers">${escape(p.choose)}</button>`;
    return `${trouble}
      <section class="cp-section"><h2 class="section-title cp-title" title="${escape(words.openCodex)}"><span>${escape(p.title)}</span>${add}</h2>
        ${legacy ? `<p class="cp-note">${escape(p.hubOld)}</p>` : ""}
        ${rows ? `<ul class="cp-rows">${rows}</ul>` : `<p class="cp-note">${escape(logins.length ? p.none : words.noLogins)}</p>`}
        ${legacy ? "" : this.inAll(collab)}</section>
      <section class="cp-section"><h2 class="section-title cp-title" title="${escape(words.callersSaid)}"><span>${escape(words.callers)}</span>${pick}</h2>${this.callers(collab)}</section>
      <section class="cp-section"><h2 class="section-title" title="${escape(words.rulesHint)}">${escape(p.rulesAll)}</h2>
        <div class="prompt-paper cp-paper"><textarea class="prompt-text cp-rules" rows="4" spellcheck="false" placeholder="${escape(words.rulesHint)}" aria-label="${escape(p.rulesAll)}">${escape(this.typed ?? collab.rules)}</textarea></div>
        <footer class="prompt-foot cp-foot"><span class="prompt-note" role="status"></span><button class="sheet-go" type="button" data-act="save-rules">${escape(words.save)}</button></footer></section>
      <section class="cp-section"><h2 class="section-title">${escape(words.now)}</h2><ul class="feature-now"></ul></section>`;
  }

  /** A preset: which it is, the name the calling model chooses it by, when
   *  it is asked, what it runs with, and its switch. */
  private row(shown: Shown, at: number, legacy: boolean): string {
    const p = this.frame.words.presets;
    const { callee } = shown;
    const kind = kindOf(callee);
    const login = this.known.find((one) => sameAgentLogin(one.login, callee.login));
    const model = callee.model ? (login?.models.find((one) => one.model === callee.model)?.display_name ?? modelName(callee.model)) : "";
    const effort = callee.effort ? effortLabel(callee.effort, this.options.english) : "";
    const use = callee.use_when || p.lines[kind];
    const toggle = legacy
      ? ""
      : `<button class="cp-switch" type="button" role="switch" aria-checked="${shown.on}" data-act="toggle" data-at="${at}" aria-label="${escape(`${p.kinds[kind]} ${callee.name}`)}"><span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>`;
    return `<li class="cp-row${shown.on || legacy ? "" : " off"}"><button class="cp-open" type="button" data-act="open" data-at="${at}">
        <span class="cp-kind">${escape(p.kinds[kind])}</span>
        <span class="cp-name">${agentGlyph(callee.login.agent, 14)}<span>${escape(callee.name)}</span></span>
        <span class="cp-use" title="${escape(use)}">${escape(use)}</span>
        <span class="cp-runs">${model ? `<span class="tag">${escape(model)}</span>` : ""}${effort ? `<span class="tag">${escape(effort)}</span>` : ""}</span>
      </button>${toggle}</li>`;
  }

  /** How many colleagues of every preset together one conversation has at
   *  work at once: a row of words under the presets, the one chosen washed. */
  private inAll(collab: Collab): string {
    const { words } = this.frame;
    const p = words.presets;
    const { chosen, choices } = atOnceChoices(atOnceInAll(collab), words.noLimit);
    const row = choices
      .map(([value, label]) => `<button type="button" class="choice${value === chosen ? " chosen" : ""}" role="radio" aria-checked="${value === chosen}" data-act="in-all" data-value="${escape(value)}">${escape(label)}</button>`)
      .join("");
    return `<div class="cp-in-all"><span class="cp-in-all-label" title="${escape(p.inAllSaid)}">${escape(p.inAll)}</span><div class="choice-options" role="radiogroup" aria-label="${escape(p.inAll)}">${row}</div></div>`;
  }

  /** The callers in one line: how many of each agent, and on which
   *  machines; the line opens the sheet they are chosen on. */
  private callers(collab: Collab): string {
    const { words } = this.frame;
    const chosen = collab.callers;
    const counts = (["claude", "codex"] as const)
      .map((agent) => ({ agent, count: chosen.filter((login) => login.agent === agent).length }))
      .filter((one) => one.count > 0)
      .map(({ agent, count }) => `<span class="cp-count" title="${escape(agentName(agent))}">${glyph(agent, 16)}<span>${count}</span></span>`)
      .join("");
    const machines = [...new Set(chosen.map((login) => login.machine))];
    const where = chosen.length ? machines.join(this.options.english ? ", " : "、") : words.presets.nobody;
    return `<button class="cp-line" type="button" data-act="callers"><span class="cp-counts">${counts}</span><span class="cp-use" title="${escape(where)}">${escape(where)}</span><span class="cp-go" aria-hidden="true">${glyph("chevron", 14)}</span></button>`;
  }

  /** Who works for whom now: the GPTs asked in that are at work. */
  private drawNow(): void {
    const list = this.frame.body.querySelector<HTMLElement>(".feature-now");
    if (!list) return;
    const { words } = this.frame;
    const residents = residentsOf(this.options.store.world.peek());
    const working = residents.filter((r) => r.about.origin.kind === "colleague" && r.now?.activity === "busy");
    const html = working.length
      ? working
          .map((r) => {
            const origin = r.about.origin.kind === "colleague" ? r.about.origin : null;
            const asker = origin ? residents.find((one) => one.about.id === origin.asked_by.resident && one.about.machine === origin.asked_by.machine) : undefined;
            const askerName = asker ? (titleOf(asker) ?? `${agentName(asker.about.host.agent)} ${asker.about.machine}`) : (origin?.asked_by.resident ?? "");
            return `<li><a href="${link({ kind: "talk", address: { resident: r.about.id, machine: r.about.machine } })}">${agentGlyph(r.about.host.agent, 14)}<span>${escape(titleOf(r) ?? r.about.id)}</span><span class="feature-now-for">${escape(words.workingFor(askerName))} ${escape(r.about.machine)}</span></a></li>`;
          })
          .join("")
      : `<li class="feature-quiet">${escape(words.nobodyNow)}</li>`;
    if (list.innerHTML !== html) list.innerHTML = html;
  }

  /** Under the rules: the way to save what is typed, and that it was. */
  private drawFoot(): void {
    const foot = this.frame.body.querySelector<HTMLElement>(".cp-foot");
    if (!foot) return;
    const { words } = this.frame;
    foot.querySelector<HTMLButtonElement>("[data-act='save-rules']")!.disabled = this.typed === null || this.busy;
    const note = foot.querySelector<HTMLElement>(".prompt-note")!;
    const html = this.said && this.typed === null ? `<span class="prof-said">${escape(words.saved)}</span>` : "";
    if (note.innerHTML !== html) note.innerHTML = html;
  }
}
