// How a machine's update stands, drawn as a mark and the version it goes to
// rather than a sentence (`world-gui.md`, 界面的结构): nothing when it is up to
// date, a download mark when it can update, a thin bar while a package comes,
// a clock while it waits, a looping clock while an update is scheduled, a
// moon until that computer's Fastctx is opened, a cross when it failed. The
// sentence is said when pointed at, or held on a phone. A group has a small
// bar of what its machines are at.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { MachineUpdate, UpdateStage } from "../world/updates";
import type { UpdateViewWords } from "./update-view-words";

export type MarkKind = "newest" | "behind" | "going" | "due" | "failed" | "quiet";

export interface Mark {
  kind: MarkKind;
  html: string;
  said: string;
  /** What went wrong, in the words of whatever said so, for the hover. */
  why?: string;
}

/** What a machine runs: its application where one carries its World
 *  program, else the program itself. */
export function runningOf(machine: MachineUpdate): string | null {
  return machine.app?.version ?? machine.kernel;
}

/** A version without the release line every version shown shares, as
 *  `beta.8` for `1.0.1-beta.8`; another line's is written whole. */
export function shortVersion(version: string, line: string | null): string {
  const match = /^(\d+\.\d+\.\d+)-(.+)$/.exec(version);
  return match && match[1] === line ? match[2]! : version;
}

/** The release line of `version`: `1.0.1` for `1.0.1-beta.8`. */
export function lineOf(version: string | null | undefined): string | null {
  return version ? (/^(\d+\.\d+\.\d+)/.exec(version)?.[1] ?? null) : null;
}

/** How an update under way or stopped stands, as a node says it: the same
 *  mark and sentence wherever a machine's update is shown, the machines'
 *  update view, a machine's own page and this device's settings; none when
 *  nothing is under way. */
export function stageMark(stage: UpdateStage, line: string | null, words: UpdateViewWords): Mark | null {
  const short = (version: string) => `<span class="ml-num">${escape(shortVersion(version, line))}</span>`;
  const with_ = (kind: MarkKind, icon: Parameters<typeof glyph>[0], version: string, said: string): Mark => ({ kind, html: `${glyph(icon, 16)}${short(version)}`, said });
  switch (stage.kind) {
    case "fetching": {
      const percent = stage.total ? Math.min(100, Math.floor((stage.received / stage.total) * 100)) : null;
      const via = stage.via ? glyph(stage.via === "github" ? "web" : "tree", 14) : "";
      return {
        kind: "going",
        html: `<span class="uv-bar"><span style="width:${percent ?? 4}%"></span></span>${via}`,
        said: `${shortVersion(stage.version, line)}　${words.fetching(percent, stage.via)}`,
      };
    }
    case "waiting_idle":
      return with_("going", "clock", stage.version, words.waitingIdle);
    case "waiting_app":
      return with_("going", "sleep", stage.version, words.waitingApp);
    case "ready":
      return with_("going", "clock", stage.version, words.ready(stage.version));
    case "installing":
      return with_("going", "update", stage.version, words.installing(stage.version));
    case "renewing":
      return with_("going", "update", stage.version, words.renewing(stage.version));
    // An earlier release's wait for an administrator: its startup service
    // takes the new program by itself now.
    case "administrator":
      return with_("going", "update", stage.version, words.administrator);
    case "failed":
      return { ...with_("failed", "cross", stage.version, words.failed(stage.version, words.failures[stage.problem.reason.kind])), why: stage.problem.why };
    case "repairing":
      return with_("going", "repair", stage.version, words.repairing((words.repairWords.layers as Record<string, string>)[stage.layer] ?? stage.layer));
  }
  return null;
}

export function markOf(machine: MachineUpdate, line: string | null, words: UpdateViewWords): Mark {
  const short = (version: string) => `<span class="ml-num">${escape(shortVersion(version, line))}</span>`;
  const with_ = (kind: MarkKind, icon: Parameters<typeof glyph>[0], version: string, said: string): Mark => ({ kind, html: `${glyph(icon, 16)}${short(version)}`, said });
  if (machine.kind === "phone") return { kind: "quiet", html: "", said: words.reasons.phone };
  const staged = stageMark(machine.stage, line, words);
  if (staged) return staged;
  // A repair kept for it until it is back, or its last one that stopped short.
  if (machine.repair_due) {
    const blocked = machine.repair_due.problem;
    return blocked
      ? { ...with_("failed", "repair", machine.repair_due.version, words.repairDueBlocked(words.failures[blocked.reason.kind])), why: blocked.why }
      : with_("due", "repair", machine.repair_due.version, words.repairDue(machine.repair_due.by));
  }
  if (machine.repair?.finished && !machine.repair.success) {
    return { ...with_("failed", "repair", machine.repair.version, words.repairFailed), why: machine.repair.error ?? "" };
  }
  if (machine.due) {
    const blocked = machine.due.problem;
    return blocked
      ? { ...with_("failed", "history", machine.due.version, words.dueBlocked(machine.due.version, words.failures[blocked.reason.kind])), why: blocked.why }
      : with_("due", "history", machine.due.version, words.due(machine.due.version, machine.due.by));
  }
  if (!runningOf(machine)) return { kind: "quiet", html: "", said: words.unknown };
  if (machine.offer) return with_("behind", "update", machine.offer.version, words.behind(machine.offer.version));
  return { kind: "newest", html: "", said: words.newest };
}

/** Whether a machine's update is under way or waiting to be made. */
export function goingOf(stage: UpdateStage): boolean {
  return stage.kind !== "idle" && stage.kind !== "unknown" && stage.kind !== "failed";
}

/** A group's small bar: up to date in ink, on the way in clay, able to
 *  update in graphite, failed in a light red; what it counts said on hover. */
export function groupMark(machines: MachineUpdate[], line: string | null, words: UpdateViewWords): Mark {
  let newest = 0;
  let going = 0;
  let behind = 0;
  let failed = 0;
  for (const machine of machines) {
    const kind = markOf(machine, line, words).kind;
    if (kind === "newest") newest += 1;
    else if (kind === "going" || kind === "due") going += 1;
    else if (kind === "behind") behind += 1;
    else if (kind === "failed") failed += 1;
  }
  const part = (count: number, kind: string) => (count ? `<span class="${kind}" style="flex:${count}"></span>` : "");
  return {
    kind: "quiet",
    html: newest + going + behind + failed ? `<span class="uv-meter">${part(newest, "newest")}${part(going, "going")}${part(behind, "behind")}${part(failed, "failed")}</span>` : "",
    said: words.group(newest, going, behind, failed),
  };
}
