// The owner's reset history: one bounded page, dates and remaining quota.
import { escape } from "../../conversation/prose";
import type { UsageReset } from "../../world/usage";
import type { UsageWords } from "./words";

const PAGE_SIZE = 40;

export function resetHistory(records: readonly UsageReset[], words: UsageWords, requestedPage: number): string {
  const pages = Math.max(1, Math.ceil(records.length / PAGE_SIZE));
  const page = Math.min(pages - 1, Math.max(0, requestedPage));
  const remaining = (used: number) => `${Number((Math.max(0, 1000 - used) / 10).toFixed(1))}%`;
  const rows = records.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE).map((record) => {
    const before = record.before_used_tenths === undefined ? null : remaining(record.before_used_tenths);
    const after = remaining(record.used_tenths);
    const description = [words.resetStarts(new Date(record.since)), words.resetConfirmed(new Date(record.confirmed_at)), words.resetEnds(new Date(record.resets_at)), record.source === "recorded" ? words.resetRecordedWhy : ""].filter(Boolean).join("\n");
    return `<li class="quota-reset-row" tabindex="0" title="${escape(description)}">
      <time datetime="${escape(record.since)}">${escape(words.resetDate(new Date(record.since)))}</time>
      <span class="quota-reset-source">${escape(words.resetSources[record.source])}</span>
      <span class="quota-reset-amount" aria-label="${escape(words.resetRemaining(before, after))}">${before === null ? "" : `<span>${escape(before)}</span><span aria-hidden="true">→</span>`}<strong>${escape(after)}</strong></span>
    </li>`;
  }).join("");
  return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.resetHistory)}</span><span class="ledger-grow"></span><span class="quota-reset-label">${escape(words.remaining)}</span></div>
    <div class="quota-reset-scroll scroller">${records.length ? `<ol class="quota-reset-list">${rows}</ol>` : `<p class="feature-quiet quota-reset-empty">${escape(words.resetsNone)}</p>`}</div>
    ${pages > 1 ? `<div class="quota-reset-pages"><button type="button" data-reset-page="${page - 1}" ${page === 0 ? "disabled" : ""}>${escape(words.resetPrevious)}</button><span>${page + 1} / ${pages}</span><button type="button" data-reset-page="${page + 1}" ${page + 1 === pages ? "disabled" : ""}>${escape(words.resetNext)}</button></div>` : ""}`;
}
