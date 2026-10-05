// What a specimen does when it is asked to act: answers that change
// nothing anywhere, so each part of the interface can be looked at in a
// plain browser.

import type { Setup } from "../world/bridge";
import type { Offers } from "../world/control";

/** What a specimen conversation offers, so its menus can be looked at. */
export const specimenOffers: Offers = {
  controls: ["command", "compact", "model", "rename", "fork", "rewind", "answer"],
  commands: [
    { name: "review", description: "Review the changes so far", args: null, source: "Codex" },
    { name: "init", description: "Write an AGENTS.md for this place", args: null, source: "Codex" },
    { name: "mention", description: "Mention a file", args: "<path>", source: "Codex" },
  ],
  models: [
    { id: "gpt-6.1-sol", name: "GPT-6.1 Sol", description: "The default", efforts: ["low", "medium", "high", "xhigh"], default_effort: "medium", default: true },
    { id: "gpt-6-astra", name: "GPT-6 Astra", description: "Deeper, slower", efforts: ["medium", "high", "xhigh"], default_effort: "high", default: false },
  ],
  model: "gpt-6.1-sol",
  effort: "medium",
  usage: {
    context: null,
    limits: [
      { name: "codex primary 300m", used_percent: 34, resets_at: "2026-10-04T18:40:00Z" },
      { name: "codex secondary 10080m", used_percent: 61, resets_at: "2026-10-08T03:00:00Z" },
    ],
    cost_usd: null,
    raw: null,
  },
};

/** Files a specimen's machine has in its project's places, found by the
 *  letters typed in order, as a fuzzy finder does. */
export async function specimenFiles(query: string) {
  const files = [
    "D:/work/mark50/src/ctrl/pid.rs",
    "D:/work/mark50/src/ctrl/fixed.rs",
    "D:/work/mark50/src/ctrl/loop.rs",
    "D:/work/mark50/src/telemetry/frame.rs",
    "D:/work/mark50/specs/控制环.md",
    "D:/work/mark50/specs/遥测.md",
    "D:/work/mark50/tests/pid_saturation.rs",
    "D:/work/mark50/Cargo.toml",
  ];
  const wanted = query.toLowerCase();
  const fits = (path: string) => {
    let at = 0;
    for (const letter of path.toLowerCase()) if (letter === wanted[at]) at += 1;
    return at === wanted.length;
  };
  await new Promise((resolve) => setTimeout(resolve, 120));
  return files.filter(fits).map((path) => ({ path }));
}

/** A specimen computer, upgraded from an old fastctx: it changes nothing
 *  anywhere, only how the settings show it. */
export function specimenComputer() {
  let kept = false;
  const after = <T>(value: T) => new Promise<T>((resolve) => setTimeout(() => resolve(value), 900));
  return {
    setup: async (): Promise<Setup> => ({
      installed: "C:/Users/me/.fastctx/bin/fastctx.exe",
      carried: null,
      same: false,
      service: kept,
      codex: "in",
      claude: "out",
      legacy: {
        kind: "done",
        record: "C:/Users/me/.fastctx/upgrade-clean.json",
        archives: ["C:/Users/me/.fastctx/legacy-archive-2026-10-04"],
        archived: 14,
        tidied: 6,
        waiting: 1,
      },
    }),
    keep: () => {
      kept = true;
      return after({ kind: "installed" as const });
    },
    release: () => {
      kept = false;
      return after({ kind: "removed" as const });
    },
  };
}
