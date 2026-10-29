// What the side of a conversation needs from the World: its plan, its goal
// and the colleagues it asked, followed as they change, what the user does
// to them, and other conversations to open beside it. Each `watch` returns
// how to stop following.

import type { PageSeams, PlanShown } from "../plan/panel";
import type { PlanWords } from "../plan/words";
import type { Address } from "../world/wire";
import type { CollabNode, GoalShown, GoalStatus, PastGoal } from "./types";
import type { SideWords } from "./words";

/** A conversation opened beside another, in a tab of its side. */
export interface OpenedTalk {
  element: HTMLElement;
  name: string;
  agent: "claude" | "codex";
  leave(): void;
  /** Its column is carried as the room beside it changes. */
  roomWillChange?(by: number): void;
  /** Opens it as the page's own conversation. */
  main(): void;
}

export interface SideSeams {
  words: SideWords;
  /** The conversation the side belongs to. */
  self: Address;
  /** A conversation shown in another's side: only its goal's bar, no side
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
    set(objective: string, seen?: number, standing?: boolean): Promise<boolean>;
    /** The conversation's earlier goals, oldest first. */
    past(): Promise<PastGoal[]>;
    pause(seen?: number): Promise<boolean>;
    resume(seen?: number): Promise<boolean>;
    clear(seen?: number): Promise<boolean>;
  };
  /** The colleagues the conversation asked, each with the host's agents
   *  it sent, as far as they are known. */
  colleagues(show: (nodes: CollabNode[]) => void): () => void;
  /** Another conversation, to talk with it beside this one: a colleague it
   *  asked, or one dragged in from the list; null where it cannot be opened
   *  here. */
  open(address: Address): OpenedTalk | null;
  /** Another conversation, opened as the page's own instead, where there is
   *  no room for two. */
  go(address: Address): void;
  /** A side conversation branched off this one, carrying its history, to
   *  talk with beside it (`world-gui.md`); `can` says whether its host
   *  branches now. Absent on a conversation shown in another's side. */
  fork?: { can(): Promise<boolean>; make(): Promise<Address> };
  /** How another conversation's goal stands, followed for its tab. */
  goalOf?(address: Address, show: (status: GoalStatus | null) => void): () => void;
}
