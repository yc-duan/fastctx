// Files the user attaches to their words, as the core describes them
// (`fastctx-types/src/attachments.rs`): kept on the sending computer, with
// a small history image at the conversation's home. Phones upload there.

import type { HubAccount } from "./accounts";
import type { Address, NoticeId, ResidentId } from "./wire";

/** Where an upload goes: an agent there now, a conversation about to be
 *  opened, or the rest of that new conversation's files. */
export type UploadTarget =
  | { kind: "resident"; resident: ResidentId }
  | { kind: "new"; agent: string; town: string | null }
  | { kind: "draft"; resident: ResidentId; town: string | null };

/** Storage policy sent across the native bridge, separate from the conversation target. */
export interface SentUploadTarget {
  kind: "sent";
  machine: string;
  target: UploadTarget;
  operation: string;
  /** The account the file is for, where the conversation works for one
   *  assisted by another's agent. */
  account?: HubAccount;
  /** The machine of that account's the original goes to, as written. */
  files_on?: string;
}

/** Where a file other than an image goes, for a conversation an agent of
 *  another's works in for the user: that account, and one of its own
 *  machines (`world-run.md`, 受协助的人附的文件). */
export interface FilePlace {
  account: HubAccount;
  filesOn: string;
}

export interface UploadFile {
  name: string;
  media_type: string;
  size: number;
  /** Lowercase hexadecimal SHA-256 of the bytes. */
  sha256: string;
}

/** A file kept for an agent, verified by size and digest. */
export interface Attachment extends UploadFile {
  id: NoticeId;
  owner: Address;
  conversation?: Address;
  town: string | null;
  path: string;
}

/** A file the user sent: as the World kept it, or a picture its agent's own
 *  host recorded with the user's words, placed there by the node, never by
 *  the page. */
export type AttachmentSource = Attachment | {
  owner: Address;
  host_image:
    | { host: "codex"; turn: string; message: number; image: number }
    | { host: "claude"; message: string; image: number };
} | {
  /** An image a tool gave back, by the tool's item and the image's place
   *  among the images it gave. */
  owner: Address;
  tool_image: { turn?: string; item: string; image: number };
};
