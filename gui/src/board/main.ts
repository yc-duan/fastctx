// The window a card of the side stands in once taken out (`side/board.ts`):
// the plan, the files, the colleagues or the host's agents of one
// conversation, and nothing else of the app. It reads nothing of the World
// itself. The side's own parts are put together here as they are in the
// desk's window, with every seam of theirs asking the desk's window
// instead: what is followed there is told here as it changes, what the
// user does here is done there. So the window is light, shows what the
// desk shows, and goes when the desk shows its conversation no more.

import "katex/dist/katex.min.css";
import "../design/fonts.css";
import "../design/tokens.css";
import "../design/breath.css";
import "../design/arrive.css";
import "../design/base.css";
import "../design/highlight.css";
import "../conversation/conversation.css";
import "../longview/longview.css";
import "../app/shell.css";
import "../app/press.css";
import "../app/sheet.css";
import "../design/head-switch.css";
import "./board.css";
import { placeOf } from "../app/places";
import { followSwitches } from "../app/segment-slide";
import { installPointerHints } from "../app/pointer-hint";
import { fileKind } from "../conversation/file-kind";
import type { FileChange } from "../files/model";
import type { Listing } from "../files/tree";
import type { FileLines } from "../files/view";
import { enFiles, zhFiles } from "../files/words";
import type { LineChange, LineExtent, LineInterruption, LineMatch } from "../longview/source";
import { installRules } from "../pen/rules";
import type { LiveFact } from "../plan/live";
import type { PlanSaved, PlanShown } from "../plan/panel";
import { enPlan, zhPlan } from "../plan/words";
import { ONLY, Remote, RemoteFailure } from "../side/board";
import { FileTabs } from "../side/file-tabs";
import { SideFrame } from "../side/frame";
import { SideParts, type ChangedTold } from "../side/parts";
import { cardNames } from "../side/shown";
import type { CollabNode, GoalShown, PastGoal, WhereShown } from "../side/types";
import { enSide, zhSide } from "../side/words";
import { applyTheme } from "../theme";

/** What the desk's window says of itself as a board first asks. */
interface Hello {
  host: "claude" | "codex";
  /** Whether a machine's folders can be listed, a folder shown in the
   *  system's own window, a picture made small, a file opened outside. */
  list: boolean;
  /** Whether a machine has a page of files of its own to go to. */
  browse: boolean;
  reveal: boolean;
  picture: boolean;
  outside: boolean;
}

const params = new URLSearchParams(location.search);
const english = (() => {
  const asked = params.get("lang");
  if (asked === "en" || asked === "zh") return asked === "en";
  try {
    return localStorage.getItem("lang") === "en";
  } catch {
    return false;
  }
})();

/** A file's lines read by the desk's window: how many there are is kept
 *  here as the desk tells it, since it is asked for without waiting. */
async function linesOf(remote: Remote, machine: string, path: string): Promise<FileLines> {
  const { handle, extent } = await remote.call<{ handle: number; extent: LineExtent }>("lines.open", machine, path);
  let now = extent;
  const changes = new Set<(change: LineChange) => void>();
  const breaks = new Set<(broken: LineInterruption | null) => void>();
  const stop = remote.watch(
    "lines",
    (what: "changed" | "broken", value: LineChange | LineInterruption | null, at?: LineExtent) => {
      if (what === "changed") {
        if (at) now = at;
        for (const heard of [...changes]) heard(value as LineChange);
      } else for (const heard of [...breaks]) heard(value as LineInterruption | null);
    },
    handle,
  );
  return {
    get extent() {
      return now;
    },
    read: (start, count) => remote.call<readonly string[]>("lines.read", handle, start, count),
    find: (query, from, direction) => remote.call<LineMatch | null>("lines.find", handle, query, from, direction),
    subscribe: (changed) => {
      changes.add(changed);
      return () => changes.delete(changed);
    },
    interruptions: (heard) => {
      breaks.add(heard);
      return () => breaks.delete(heard);
    },
    close: async () => {
      stop();
      await remote.call("lines.close", handle);
    },
  };
}

async function open(): Promise<void> {
  const place = placeOf(location.hash);
  if (!ONLY || place.kind !== "talk") return;
  const address = place.address;
  const remote = new Remote(address, ONLY);
  const words = english ? enSide : zhSide;
  const fileWords = english ? enFiles : zhFiles;
  document.documentElement.classList.add("side-board");
  document.title = `${cardNames(words)[ONLY]} - Fastctx`;
  installPointerHints();
  installRules();
  // A switch's chosen side slides to the one pressed here too, as on the desk.
  followSwitches();
  applyTheme(() => {});
  const hello = await remote.call<Hello>("hello");
  const detail = (error: unknown) => (error instanceof Error ? error.message : String(error));

  /** What is known of each id the plan names, as the desk last told it. */
  let facts: Record<string, LiveFact | null> = {};
  /** The files the conversation changed, as the desk last told them: the
   *  lines of a change are asked for when it is opened. */
  let changed: ReturnType<typeof restored> = [];
  const restored = (told: ChangedTold) =>
    told.map((file) => ({
      ...file,
      changes: file.changes.map(({ rowed, ...change }): FileChange => ({ ...change, ...(rowed ? { rows: () => remote.call("rows", change.id) } : {}) })),
    }));

  const parts: SideParts = new SideParts(document.createElement("div"), {
    host: hello.host,
    seams: {
      words,
      self: address,
      // The goal stands on its bar above the input, in the desk's window.
      noGoal: true,
      plan: {
        words: english ? enPlan : zhPlan,
        seams: {
          facts: (id) => facts[id] ?? null,
          open: (id) => void remote.call("plan.open", id),
          save: (source, expected) => remote.call<PlanSaved>("plan.save", source, expected),
        },
        watch: (show) =>
          remote.watch("plan", (plan: PlanShown | null, known: Record<string, LiveFact | null>) => {
            facts = known;
            show(plan);
          }),
      },
      goal: {
        watch: (show) => remote.watch("goal", (goal: GoalShown | null) => show(goal)),
        set: () => Promise.resolve(false),
        past: () => Promise.resolve<PastGoal[]>([]),
        pause: () => Promise.resolve(false),
        resume: () => Promise.resolve(false),
        clear: () => Promise.resolve(false),
      },
      // Everyone who works with it, as the desk's side has them: the
      // colleagues and the host's agents already as one.
      colleagues: (show) => ({ stop: remote.watch("team", (nodes: CollabNode[]) => show(nodes)), detail: () => {} }),
      where: (show) => remote.watch("where", (where: WhereShown | null) => show(where)),
      ...(hello.reveal ? { reveal: (path: string) => void remote.call("reveal", path) } : {}),
      files: {
        words: fileWords,
        list: hello.list ? (machine, path) => remote.call<Listing>("files.list", machine, path) : null,
        home: (machine) => remote.call<string | null>("files.home", machine),
        // The machine's own page of files is the desk's window's to show.
        ...(hello.browse ? { browse: (machine: string, folder: string | null) => void remote.call("files.browse", machine, folder) } : {}),
      },
    },
    say: (text, said) => void remote.call("say", text, said ?? null),
    answer: (text) => remote.call<void>("answer", text),
    // One of the host's agents is read in the desk's window (`away`).
    jump: () => {},
    openFile: (machine, path, tab) => {
      if (!tabs.turn(machine, path, tab)) tabs.open(machine, path, tab);
    },
    detail,
    away: { node: (node) => void remote.call("node", node.id) },
  });
  const tabs = new FileTabs({
    side: () => parts,
    words: fileWords,
    reading: {
      lines: (machine, path) => linesOf(remote, machine, path),
      picture: hello.picture ? (machine, path, max) => remote.call<string>("picture", machine, path, max) : undefined,
      outside: hello.outside ? (machine, path) => void remote.call("outside", machine, path) : undefined,
      gone: (error) => error instanceof RemoteFailure && error.gone,
      detail,
    },
    changes: (machine, path) => changed.find((file) => file.machine === machine && file.path === path)?.changes ?? [],
    // A file a link names: read here where it is words, else opened by
    // the desk's window as it would be there.
    open: (machine, path) => {
      if (fileKind(path) !== "text") void remote.call("outside", machine, path);
      else if (!tabs.turn(machine, path, null)) tabs.open(machine, path, null);
    },
  });
  remote.watch("changed", (told: ChangedTold) => {
    changed = restored(told);
    parts.changed(changed);
    tabs.update();
  });

  const host = document.createElement("div");
  host.className = "sb-host";
  const main = document.createElement("div");
  main.className = "sb-main";
  document.getElementById("app")!.replaceChildren(host);
  new SideFrame(host, main, words, { beside: () => {} }).use(parts);
}

void open();
