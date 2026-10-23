// What the panel beside a conversation needs from the World: its plan, its
// goal and the colleagues it asked, followed as they change, and what the
// user does to them. Each `watch` returns how to stop following.

import type { PageSeams, PlanShown } from "../plan/panel";
import type { PlanWords } from "../plan/words";
import type { CollabNode, GoalShown } from "./types";
import type { SideWords } from "./words";

export interface SideSeams {
  words: SideWords;
  /** A colleague's page in another's panel: only its goal's bar, no panel
   *  of its own. */
  bare?: boolean;
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
    set(objective: string, seen?: number): Promise<boolean>;
    pause(seen?: number): Promise<boolean>;
    resume(seen?: number): Promise<boolean>;
    clear(seen?: number): Promise<boolean>;
  };
  /** The colleagues the conversation asked, each with the host's agents
   *  it sent, as far as they are known. */
  colleagues(show: (nodes: CollabNode[]) => void): () => void;
  /** A colleague's own conversation, to talk with it in the panel; null
   *  where it cannot be opened here. */
  colleague(node: CollabNode): { element: HTMLElement; leave(): void } | null;
}
