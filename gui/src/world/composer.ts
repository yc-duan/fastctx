import type { Attachment, UploadFile } from "./attachments";
import type { Landed } from "./bridge";
import type { MachineName, NoticeId } from "./wire";
import type { Result } from "./wire";

/** Legacy identity-edit contract. Indices count Unicode scalars. */
export interface CharacterId {
  readonly operation: string;
  readonly index: number;
}

export interface Character {
  readonly id: CharacterId;
  readonly text: string;
}

export interface ComposerDraft {
  readonly epoch: number;
  readonly text: string;
  readonly characters: readonly Character[];
  readonly attachments: readonly Attachment[];
  /** `operation` is the upload operation it arrives under; older nodes do not say. */
  readonly uploads: readonly { id: NoticeId; file: UploadFile; error: string | null; operation?: string }[];
  readonly files_on: string | null;
}

export interface ComposerQueued {
  readonly id: NoticeId;
  readonly from: MachineName;
  readonly draft: ComposerDraft;
  readonly editing: boolean;
  readonly sending: boolean;
  readonly error: string | null;
}

/** A command the node took to do once a turn ends, where it stands among
 *  the words that wait: after `after`, or before them all. It is moved as
 *  they are, by its id. */
export interface ComposerCommand {
  readonly id: NoticeId;
  readonly after: NoticeId | null;
}

export interface ComposerSnapshot {
  readonly revision: number;
  readonly draft: ComposerDraft;
  readonly queue: readonly ComposerQueued[];
  /** Absent where the node keeps no commands among the words. */
  readonly commands?: readonly ComposerCommand[];
  readonly unobserved: readonly ComposerQueued[];
  readonly held: string | null;
  readonly awaiting_turn: string | null;
}

export type ComposerTarget = { readonly kind: "draft" } | { readonly kind: "queued"; readonly id: NoticeId };

export interface ComposerEdit {
  readonly operation: string;
  readonly do: "edit";
  readonly target: ComposerTarget;
  readonly epoch: number;
  readonly after: CharacterId | null;
  readonly delete: readonly CharacterId[];
  readonly insert: string;
}

export interface ComposerUpdated {
  readonly composer: ComposerSnapshot;
  readonly landed: Landed | null;
  /** The id the words this change sent are kept under on their way in, as
   *  the conversation's `now` names them; older nodes do not say. */
  readonly delivery?: NoticeId;
}

export type ComposerChange =
  | Omit<ComposerEdit, "operation">
  | { readonly do: "attach"; readonly target: ComposerTarget; readonly epoch: number; readonly attachment: Attachment }
  | { readonly do: "detach"; readonly target: ComposerTarget; readonly epoch: number; readonly attachment: NoticeId }
  | { readonly do: "files_on"; readonly target: ComposerTarget; readonly epoch: number; readonly files_on: string | null }
  | { readonly do: "send"; readonly epoch: number }

  | { readonly do: "editing"; readonly id: NoticeId; readonly watch: string; readonly editing: boolean }
  | { readonly do: "delete"; readonly id: NoticeId }
  | { readonly do: "move"; readonly id: NoticeId; readonly before: NoticeId | null }
  | { readonly do: "send_queued"; readonly id: NoticeId };

export type ComposerMutation = ComposerChange & { readonly operation: string };

export interface ComposerUploadScope {
  readonly operation: string;
  readonly target: ComposerTarget;
  readonly epoch: number;
}

export interface ComposerAttachmentAdded {
  readonly attachment: Attachment;
  readonly updated: Result<ComposerUpdated, string>;
}

export type ComposerPiece =
  | { readonly kind: "snapshot"; readonly watch: string; readonly composer: ComposerSnapshot }
  | { readonly kind: "interrupted"; readonly error: string | null; readonly since: number };

export interface ComposerWatching {
  readonly id: number;
  readonly watch: string;
  readonly composer: ComposerSnapshot;
}
