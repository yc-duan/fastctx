// What a conversation has used, over its input (`world-gui.md`, 对话), set
// out as one page of a notebook. At the top the three figures of the
// input's row, in its order and with its marks, said once each and large.
// Under them how the cache served each request, with the whole conversation
// small beneath to choose a stretch from (`hit-chart.ts`), and what that
// stretch's prompts and answers were made of, one bar of marker, and the way
// to each step's context on a page of its own (`context-sheet.ts`). At the
// foot the account's windows as rings, and what the conversation has cost.
// All of it is what the conversation already holds or its host last pushed.

import type { ComposerWords } from "../app/words";
import { parts, ring } from "../app/usage/draw";
import type { Usage } from "../world/control";
import { glyph, type GlyphName } from "./glyphs";
import { HitChart } from "./hit-chart";
import { escape } from "./prose";
import { contextOf, hitOf, warmUntil, type ModelRequest } from "./requests";
import { percentFine, tokensShort } from "./usage-badge";
import { inkMark } from "./usage-ink";

type Words = ComposerWords["usage"];

export interface UsageNow {
  context: { used: number; window: number | null } | null;
  usage: Usage | null;
  requests: readonly ModelRequest[];
}

const KINDS = ["read", "write", "fresh", "output"] as const;

/** The inks the account's usage page names the same parts with. */
const INKS: Record<(typeof KINDS)[number], string> = {
  read: "var(--pen-graphite)",
  write: "var(--pen-sky)",
  fresh: "var(--pen-sage)",
  output: "var(--pen-clay)",
};

/** The width the bars are drawn at; they stretch to the pop-up's own. */
const BAR = 448;

export class UsagePop {
  private readonly chart: HitChart;

  constructor(
    private readonly pop: HTMLElement,
    private readonly words: Words,
    /** Opens each step's context, where the page can. */
    private readonly steps?: () => void,
  ) {
    this.chart = new HitChart(words, (requests) => this.drawSpend(requests));
    pop.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-steps]")) this.steps?.();
    });
  }

  /** Draws it all; false where there is nothing to say. */
  fill(now: UsageNow): boolean {
    const { words } = this;
    const { context, usage, requests } = now;
    const used = context ?? usage?.context ?? null;
    const latest = requests[requests.length - 1] ?? null;
    const holds = used?.used ?? (latest ? contextOf(latest) : 0);
    const figures: string[] = [];
    if (holds) {
      const share = used?.window ? ` · ${Math.round((holds / used.window) * 100)}%` : "";
      figures.push(figure("context", tokensShort(holds), `${words.context}${share}`, words.contextTitle(holds, used?.window ?? null)));
    }
    if (latest && contextOf(latest)) {
      const hit = percentFine(hitOf(latest));
      figures.push(figure("cache", hit, words.hit, words.hitTitle(hit)));
    }
    const warm = warmUntil(requests);
    if (warm) {
      const left = warm.until - Date.now();
      figures.push(
        left > 0
          ? figure("clock", words.left(left), words.warm, words.warmTitle(words.left(left), words.ttl(warm.ttl)))
          : figure("clock", words.cold, words.ttl(warm.ttl), words.coldTitle, "cold"),
      );
    }
    const sections: string[] = [];
    // Each step's context is worth a page once there are steps between
    // requests.
    const steps =
      this.steps && requests.length > 1
        ? `<button class="text-button talk-usage-steps" type="button" data-steps>${glyph("context", 15)}<span>${escape(words.steps)}</span>${glyph("chevron", 12)}</button>`
        : "";
    if (requests.length) sections.push(`<div class="talk-usage-chart"></div><div class="talk-usage-spend"></div>${steps}`);
    const foot = [this.limits(usage), cost(usage, words)].join("");
    if (foot) sections.push(`<div class="talk-usage-foot">${foot}</div>`);
    if (!figures.length && !sections.length) {
      this.pop.innerHTML = `<div class="usage-title">${escape(words.title)}</div><div class="menu-empty">${escape(words.none)}</div>`;
      return true;
    }
    const head = figures.length ? `<div class="talk-usage-figures">${figures.join("")}</div>` : `<div class="usage-title">${escape(words.title)}</div>`;
    this.pop.innerHTML = head + sections.map((section) => `<div class="talk-usage-rule" aria-hidden="true"></div>${section}`).join("");
    if (requests.length) {
      this.pop.querySelector(".talk-usage-chart")!.replaceWith(this.chart.element);
      this.chart.update(requests);
      this.drawSpend(this.chart.visible());
      // Drawn again at its width once the pop-up stands open.
      requestAnimationFrame(() => this.chart.update(requests));
    }
    return true;
  }

  /** What the prompts and answers of the stretch shown were made of: one
   *  bar of marker, its inks named under it with how much. */
  private drawSpend(requests: readonly ModelRequest[]): void {
    const box = this.pop.querySelector<HTMLElement>(".talk-usage-spend");
    if (!box) return;
    const { words } = this;
    const sums = { read: 0, write: 0, fresh: 0, output: 0 };
    for (const request of requests) for (const kind of KINDS) sums[kind] += request[kind];
    const shown = KINDS.filter((kind) => sums[kind]);
    const span = requests.length > 1 ? requests[requests.length - 1]!.at - requests[0]!.at : 0;
    const bar = shown.length
      ? parts(
          shown.map((kind) => ({ value: sums[kind], ink: INKS[kind], said: `${words.kinds[kind]} ${sums[kind].toLocaleString()}` })),
          `talk:${requests.length}:${requests[0]?.at ?? 0}`,
          BAR,
        )
      : "";
    box.innerHTML = `<div class="talk-usage-stretch">${escape(words.requests(requests.length, words.span(span)))}</div>${bar}<div class="talk-usage-keys">${shown
      .map((kind) => `<span class="talk-usage-key">${inkMark(INKS[kind], kind)}<span>${escape(words.kinds[kind])}</span><b>${tokensShort(sums[kind])}</b></span>`)
      .join("")}</div>`;
  }

  /** The account's windows, a ring each: how much is used, and when it
   *  resets; how long ago the host said so where the pointer rests. */
  private limits(usage: Usage | null): string {
    if (!usage?.limits.length) return "";
    const { words } = this;
    const minutes = usage.limits_at ? Math.max(0, Math.floor((Date.now() - new Date(usage.limits_at).getTime()) / 60000)) : null;
    const asOf = minutes === null ? "" : words.asOf(minutes);
    return usage.limits
      .map((limit) => {
        const percent = limit.used_percent === null ? null : Number(limit.used_percent);
        const share = percent === null ? null : percent / 100;
        const resets = limit.resets_at ? new Date(limit.resets_at) : null;
        const reset = resets ? words.resetsIn(Math.max(0, resets.getTime() - Date.now())) : "";
        const said = [words.window(limit.name), percent === null ? "" : words.used(String(percent)), resets ? words.resets(resets) : "", asOf].filter(Boolean).join(" · ");
        return `<span class="talk-usage-ring" title="${escape(said)}"><span class="talk-usage-ring-face">${ring(share, `talk:${limit.name}`, 44)}<span class="talk-usage-ring-num"><span>${percent === null ? "–" : `${Math.round(percent)}<small>%</small>`}</span></span></span><span class="talk-usage-ring-text"><span class="talk-usage-ring-name">${escape(words.window(limit.name))}</span><span class="talk-usage-ring-when">${escape(reset)}</span></span></span>`;
      })
      .join("");
  }
}

/** What the conversation has cost, as its host reckons it. */
function cost(usage: Usage | null, words: Words): string {
  if (usage?.cost_usd === null || usage?.cost_usd === undefined) return "";
  return `<span class="talk-usage-cost" title="${escape(words.costWhy)}">${escape(words.cost(Number(usage.cost_usd)))}</span>`;
}

/** One of the figures at the top: large, with its mark from the input's
 *  row and what it is under it. */
function figure(mark: GlyphName, value: string, label: string, title = "", state = ""): string {
  return `<div class="talk-usage-figure${state ? ` ${state}` : ""}"${title ? ` title="${escape(title)}"` : ""}><span class="talk-usage-big">${escape(value)}</span><span class="talk-usage-label">${glyph(mark, 14)}<span>${escape(label)}</span></span></div>`;
}
