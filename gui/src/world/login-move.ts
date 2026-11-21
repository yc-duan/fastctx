// A login's directory on a server moved to another disk by its Fastctx,
// asked from here over SSH (`gui/src-tauri/src/remote_world/ssh.rs`,
// `move_login`; `fastctx move-login` there).

export type MoveAsk =
  | { do: "move"; login: string; target: string; stop_agents?: boolean }
  | { do: "resume" }
  | { do: "drop_backup"; op: string; anyway?: boolean }
  | { do: "status" };

/** A move as the server records it (`fastctx move-login --status --json`). */
export interface MoveRecord {
  op: string;
  agent: string;
  login: string;
  source: string;
  target: string;
  backup: string;
  phase: "stopping" | "checking" | "copying" | "verifying" | "published" | "swapping" | "linked" | "done" | "aborted";
  service_restored: boolean;
  backup_deleted: boolean;
  why?: string | null;
  at: string;
}

export interface Moved {
  /** The command's exit; none where only the records were read. */
  status: number | null;
  records: MoveRecord[];
}

/** Whether a move leaves something to do there: its directory between its
 *  two places, or its service not put back. */
export const pending = (record: MoveRecord): boolean =>
  (record.phase !== "done" && record.phase !== "aborted") || !record.service_restored;
