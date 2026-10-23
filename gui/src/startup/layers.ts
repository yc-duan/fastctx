// A repair's layers as it goes through them, one row each: a breathing dot
// while it checks one, a tick once it is fine, a tick in clay where it was
// mended, a cross where it could not be. How each went is said when pointed
// at, with the repair's own words after it.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { StartupWords } from "./words";

export type LayerOutcome = "checking" | "unchanged" | "repaired" | "failed";

export interface RepairStep {
  layer: string;
  outcome: LayerOutcome;
  detail: string;
  actions: string[];
}

export interface RepairReport {
  id: string;
  version: string;
  finished: string | null;
  steps: RepairStep[];
  success: boolean;
  unchanged: boolean;
  error: string | null;
}

export function repairLayers(steps: readonly RepairStep[], words: StartupWords): string {
  const rows = steps.map((step) => {
    const name = (words.layers as Record<string, string>)[step.layer] ?? step.layer;
    const said =
      step.outcome === "checking" ? words.checking : step.outcome === "unchanged" ? words.unchanged : step.outcome === "repaired" ? words.fixed : words.broken;
    const why = step.detail ? words.detail(said, step.detail) : said;
    const mark = step.outcome === "checking" ? '<span class="rl-pulse"></span>' : glyph(step.outcome === "failed" ? "cross" : "check", 16);
    return `<li class="rl-row ${step.outcome}" title="${escape(why)}"><span class="rl-mark" aria-hidden="true">${mark}</span><span class="rl-name">${escape(name)}</span></li>`;
  });
  return `<ol class="rl-layers">${rows.join("")}</ol>`;
}
