// Files the user attaches to their words, as the core describes them
// (`fastctx-types/src/attachments.rs`): uploaded to the agent's machine,
// kept in its own place, and handed to the agent as a path (or, for a
// Codex image, shown to it).

import type { Address, NoticeId, ResidentId } from "./wire";

/** Where an upload goes: an agent there now, a conversation about to be
 *  opened, or the rest of that new conversation's files. */
export type UploadTarget =
  | { kind: "resident"; resident: ResidentId }
  | { kind: "new"; agent: string; town: string | null }
  | { kind: "draft"; resident: ResidentId; town: string | null };

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
  town: string | null;
  path: string;
}

/** A kept file or one user picture in its resident's native host record. */
export type AttachmentSource = Attachment | {
  owner: Address;
  host_image:
    | { host: "codex"; turn: string; message: number; image: number }
    | { host: "claude"; message: string; image: number };
};
