// What the side of a conversation needs from the World: its plan, its goal
// and the colleagues it asked, followed as they change, and what the user
// does to them. Each `watch` returns how to stop following.

import type { PageSeams, PlanShown } from "../plan/panel";
import type { HistoryWords } from "../project/history-words";
import type { HistoryCatalog } from "../world/history";
import type { PlanWords } from "../plan/words";
import type { Address } from "../world/wire";
import type { CollabNode, GoalShown, PastGoal, WhereShown } from "./types";
import type { SideWords } from "./words";

/** The colleagues followed: stopped, or asked for more or less of them. */
export interface Following {
  stop(): void;
  /** Whether a page shows the colleagues' goals and agents now. */
  detail(shown: boolean): void;
}

export interface SideSeams {
  words: SideWords;
  /** The conversation the side belongs to. */
  self: Address;
  /** No goal is set here: a Codex conversation the user opened in Codex's
   *  own app, editor or command line, whose turns the World neither sees
   *  nor starts (`world-goal.md`). */
  noGoal?: boolean;
  plan: {
    words: PlanWords;
    /** The plan's seams but its answers, which go in as the user's words. */
    seams: Omit<PageSeams, "answer">;
    watch(show: (plan: PlanShown | null) => void): () => void;
  };
  goal: {
    watch(show: (goal: GoalShown | null) => void): () => void;
    /** Sets the goal or its words anew, opening one done again. `seen` is
     *  the version the user began from; each resolves false when the goal
     *  changed elsewhere first. */
    set(objective: string, seen?: number, standing?: boolean): Promise<boolean>;
    /** The conversation's earlier goals, oldest first. */
    past(): Promise<PastGoal[]>;
    pause(seen?: number): Promise<boolean>;
    resume(seen?: number): Promise<boolean>;
    clear(seen?: number): Promise<boolean>;
  };
  /** The colleagues the conversation asked: who each is and how it
   *  stands, as the World is pushed; with `detail`, each one's goal and the
   *  host's agents it sent too, read only while a page shows them. */
  colleagues(show: (nodes: CollabNode[]) => void): Following;
  /** Where it works, followed as the World changes. */
  where(show: (where: WhereShown | null) => void): () => void;
  /** Shows a folder of this computer in the system's own window, where this
   *  is a computer. */
  reveal?: (path: string) => void;
  /** The history its model works with (`world-history.md`): its project's,
   *  or, in no project, its own. */
  history?: HistorySeam & {
    words: HistoryWords;
    /** The project whose history it is, for one in a project. */
    project?: { name: string; href: string };
    /** Its own history from before it joined its project. */
    earlier?: HistorySeam;
  };
}

/** One history, followed as it changes. */
export interface HistorySeam {
  watch(show: (catalog: HistoryCatalog | null) => void): () => void;
  /** Where a document is read. */
  link(name: string): string;
  write(): void;
}
