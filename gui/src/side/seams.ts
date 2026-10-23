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
  plan: {
    words: PlanWords;
    seams: PageSeams;
    watch(show: (plan: PlanShown | null) => void): () => void;
  };
  goal: {
    watch(show: (goal: GoalShown | null) => void): () => void;
    /** Sets the goal, or its words anew. */
    set(objective: string): Promise<void>;
    pause(): Promise<void>;
    resume(): Promise<void>;
    clear(): Promise<void>;
  };
  /** The colleagues the conversation asked, each with the host's agents
   *  it sent, as far as they are known. */
  colleagues(show: (nodes: CollabNode[]) => void): () => void;
  /** A colleague's own conversation, to talk with it in the panel; null
   *  where it cannot be opened here. */
  colleague(node: CollabNode): { element: HTMLElement; leave(): void } | null;
}
