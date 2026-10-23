// A conversation's plan, read and written through the host's World
// (`world-plan.md`). The hub keeps the plan, so these reach it even while the
// agent's own machine is offline; only metadata is pushed, and the page reads
// the Markdown when it changes.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Address, MachineName, ResidentId, Timestamp } from "./wire";

/** Who last wrote the plan: the person here, or the conversation itself. */
export type PlanAuthor = { kind: "user" } | { kind: "resident"; address: Address };

export interface PlanMetadata {
  kind: string;
  key: string;
  version: number;
  /** The machine the hub saw the write come from. */
  by: MachineName;
  at: Timestamp;
  deleted: boolean;
}

export interface PlanBody {
  markdown: string;
  author: PlanAuthor;
}

export interface PlanPage {
  owner: ResidentId;
  metadata: PlanMetadata | null;
  body: PlanBody | null;
}

export type PlanResult = { result: "page"; page: PlanPage } | { result: "conflict"; current: PlanMetadata | null };

export type PlanPiece =
  | { kind: "changed"; metadata: PlanMetadata | null }
  | { kind: "interrupted"; error: string; since: number };

/** The account a plan is kept for, by its hub's name; none lets the World
 *  pick this computer's only one. */
export interface PlanAccount {
  hub: string;
  account: string;
}

/** The plan as it stands, for the conversation whose resident is `owner`. */
export function planRead(owner: ResidentId, at: PlanAccount | null = null): Promise<PlanPage> {
  return invoke<PlanPage>("plan_read", { owner, hub: at?.hub ?? null, account: at?.account ?? null });
}

/** Puts the edited plan up as the next version. `expected` is the version the
 *  edit began at; a newer version in the meantime comes back as a conflict. */
export function planWrite(owner: ResidentId, expected: number | null, markdown: string, at: PlanAccount | null = null): Promise<PlanResult> {
  return invoke<PlanResult>("plan_write", { owner, expected, markdown, hub: at?.hub ?? null, account: at?.account ?? null });
}

/** Follows the plan: its metadata now, then after each change. The id resolves
 *  for `unwatch`; the body is read with `planRead` when a change says so. */
export function watchPlan(owner: ResidentId, onPiece: (piece: PlanPiece) => void, at: PlanAccount | null = null): Promise<number> {
  const pieces = new Channel<PlanPiece>();
  pieces.onmessage = (piece) => {
    try {
      onPiece(piece);
    } catch (error) {
      console.error("a plan change could not be shown", piece, error);
    }
  };
  return invoke<number>("plan_watch", { owner, pieces, hub: at?.hub ?? null, account: at?.account ?? null });
}
