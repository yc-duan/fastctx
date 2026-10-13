// What a conversation has used, over its input (`world-gui.md`, 对话): at the
// top the three figures of the input's row, said once each and large — how
// much of the last prompt the cache served, how much the conversation
// holds, how long the cache stays; under them how the cache served each
// request (`hit-chart.ts`), and what the prompts and answers of the stretch
// shown were made of; then the context in its parts where the host breaks
// it down, the account's limits, and what the conversation has cost. All
// of it is what the conversation already holds or its host last pushed.

import type { ComposerWords } from "../app/words";
import type { Usage } from "../world/control";
import { HitChart } from "./hit-chart";
import { escape } from "./prose";
import { contextOf, hitOf, warmUntil, type ModelRequest } from "./requests";
import { percentFine, tokensShort } from "./usage-badge";
import { usageDetail } from "./usage-detail";

type Words = ComposerWords["usage"];

export interface UsageNow {
  context: { used: number; window: number | null } | null;
  usage: Usage | null;
  requests: readonly ModelRequest[];
}

const KINDS = ["read", "write", "fresh", "output"] as const;

export class UsagePop {
  private readonly chart: HitChart;

  constructor(
    private readonly pop: HTMLElement,
    private readonly words: Words,
  ) {
    this.chart = new HitChart(words, (requests) => this.drawSpend(requests));
  }

  /** Draws it all; false where there is nothing to say. */
  fill(now: UsageNow): boolean {
    const { words } = this;
    const { context, usage, requests } = now;
    const detail = usageDetail(usage?.raw);
    const used = context ?? usage?.context ?? null;
    const latest = requests[requests.length - 1] ?? null;
    const figures: string[] = [];
    if (latest && contextOf(latest)) figures.push(figure(percentFine(hitOf(latest)), words.hit));
    const holds = used?.used ?? (latest ? contextOf(latest) : 0);
    if (holds) figures.push(figure(tokensShort(holds), used?.window ? `${words.context} · ${Math.round((holds / used.window) * 100)}%` : words.context, used?.window ? words.contextUsed(holds, used.window) : ""));
    const warm = warmUntil(requests);
    if (warm) {
      const left = warm.until - Date.now();
      figures.push(left > 0 ? figure(words.left(left), words.warm, words.warmTitle(words.left(left), words.ttl(warm.ttl)), "", words.ttl(warm.ttl)) : figure(words.cold, words.ttl(warm.ttl), words.coldTitle, "cold"));
    }
    const rows: string[] = [];
    // Where the host breaks the context down, the bar is drawn in its
    // parts, each a shade of the same ink, named beneath.
    const window = used?.window ?? 0;
    const parts = window ? detail.parts : [];
    if (parts.length) {
      const tone = (at: number) => Math.round(100 - (at * 75) / Math.max(1, parts.length - 1));
      rows.push(
        `<div class="usage-row"><span class="usage-name">${escape(words.context)}</span><span class="usage-bar usage-stack">${parts.map((part, at) => `<span style="width:${Math.min(100, (part.tokens / window) * 100).toFixed(2)}%;--tone:${tone(at)}%"></span>`).join("")}</span><span class="usage-parts">${parts.map((part, at) => `<span class="usage-part"><i style="--tone:${tone(at)}%"></i><span>${escape(words.part(part.name))}</span><span>${part.tokens.toLocaleString()}</span></span>`).join("")}</span></div>`,
      );
    } else if (used && !figures.length) {
      rows.push(`<div class="usage-row"><span class="usage-name">${escape(words.context)}</span><span class="usage-fact">${escape(words.contextUsed(used.used, used.window))}</span></div>`);
    }
    if (usage?.limits.length) {
      const minutes = usage.limits_at ? Math.max(0, Math.floor((Date.now() - new Date(usage.limits_at).getTime()) / 60000)) : null;
      rows.push(
        `<div class="usage-row talk-usage-limits">${minutes !== null ? `<span class="usage-name">${escape(words.limits)}<span class="talk-usage-asof">${escape(words.asOf(minutes))}</span></span>` : ""}${usage.limits
          .map((limit) => {
            const percent = limit.used_percent;
            return `<span class="talk-usage-limit"><span class="talk-usage-limit-name">${escape(words.window(limit.name))}</span><span class="usage-bar"><span style="width:${Math.min(100, Number(percent ?? 0))}%"></span></span><span class="usage-fact">${percent !== null ? escape(words.used(String(percent))) : ""}${limit.resets_at ? ` · ${escape(words.resets(new Date(limit.resets_at)))}` : ""}</span></span>`;
          })
          .join("")}</div>`,
      );
    }
    if (usage?.cost_usd !== null && usage?.cost_usd !== undefined) {
      rows.push(`<div class="usage-row" title="${escape(words.costWhy)}"><span class="usage-fact">${escape(words.cost(Number(usage.cost_usd)))}</span></div>`);
    }
    if (!figures.length && !rows.length) {
      this.pop.innerHTML = `<div class="usage-title">${escape(words.title)}</div><div class="menu-empty">${escape(words.none)}</div>`;
      return true;
    }
    const charted = requests.length > 1;
    this.pop.innerHTML = `${figures.length ? `<div class="talk-usage-figures">${figures.join("")}</div>` : `<div class="usage-title">${escape(words.title)}</div>`}${charted ? `<div class="talk-usage-chart"></div><div class="talk-usage-spend"></div>` : ""}${rows.join("")}`;
    if (charted) {
      this.pop.querySelector(".talk-usage-chart")!.replaceWith(this.chart.element);
      this.chart.update(requests);
      this.drawSpend(this.chart.visible());
      // Drawn again at its width once the pop-up stands open.
      requestAnimationFrame(() => this.chart.update(requests));
    }
    return true;
  }

  /** What the prompts and answers of the stretch shown were made of: one
   *  bar in four inks, each named with how much. */
  private drawSpend(requests: readonly ModelRequest[]): void {
    const box = this.pop.querySelector<HTMLElement>(".talk-usage-spend");
    if (!box) return;
    const { words } = this;
    const sums = { read: 0, write: 0, fresh: 0, output: 0 };
    for (const request of requests) for (const kind of KINDS) sums[kind] += request[kind];
    const total = KINDS.reduce((sum, kind) => sum + sums[kind], 0);
    const span = requests.length > 1 ? requests[requests.length - 1]!.at - requests[0]!.at : 0;
    // A part too small to see is drawn a hair wide, so a named part is
    // never missing from its bar.
    const width = (n: number) => (total ? Math.max(n ? 0.4 : 0, (n / total) * 100) : 0);
    box.innerHTML = `<span class="talk-usage-stretch">${escape(words.requests(requests.length, words.span(span)))}</span>
      <span class="talk-usage-mix">${KINDS.map((kind) => (sums[kind] ? `<span class="${kind}" style="width:${width(sums[kind]).toFixed(2)}%"></span>` : "")).join("")}</span>
      <span class="talk-usage-keys">${KINDS.filter((kind) => sums[kind])
        .map((kind) => `<span class="talk-usage-key"><i class="${kind}"></i>${escape(words.kinds[kind])}<b>${tokensShort(sums[kind])}</b></span>`)
        .join("")}</span>`;
  }
}

/** One of the figures at the top: large, with what it is under it. */
function figure(value: string, label: string, title = "", state = "", more = ""): string {
  return `<div class="talk-usage-figure${state ? ` ${state}` : ""}"${title ? ` title="${escape(title)}"` : ""}><span class="talk-usage-big">${escape(value)}</span><span class="talk-usage-label">${escape(label)}${more ? `<span class="talk-usage-label-more"> · ${escape(more)}</span>` : ""}</span></div>`;
}
