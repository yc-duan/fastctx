import type { Attachment } from "./attachments";
import type { ComposerCommand, ComposerDraft } from "./composer";
import type { MachineName, NoticeId } from "./wire";

/** Yjs v1 bytes in JSON; the document binding decodes them to Uint8Array. */
export type DocumentBytes = string;
export interface NativeWatching { readonly id: number; readonly watch: string; readonly frame: DocumentFrame }
export type NativePiece = { readonly kind: "native"; readonly watch: string; readonly frame: DocumentFrame } | { readonly kind: "interrupted"; readonly error: string | null; readonly since: number };

export interface DocumentSend {
  readonly operation: string;
  readonly do: "send_document";
  readonly document: string;
  readonly epoch?: number;
  readonly snapshot: DocumentBytes;
  readonly take?: DocumentBytes;
  readonly attachments: readonly NoticeId[];
}
export interface DocumentVector { readonly document: string; readonly vector: DocumentBytes }
export interface DocumentSubscription { readonly version: 1; readonly known: readonly DocumentVector[] }
export interface SharedDocument {
  readonly id: string;
  readonly revision: number;
  readonly epoch: number;
  readonly attachments: readonly Attachment[];
  readonly uploads: ComposerDraft["uploads"];
  readonly files_on: string | null;
}
export interface SharedQueued {
  readonly id: NoticeId;
  readonly from: MachineName;
  readonly document: SharedDocument;
  readonly editing: boolean;
  readonly sending: boolean;
  readonly error: string | null;
}
export interface SharedComposer {
  readonly revision: number;
  readonly draft: SharedDocument;
  readonly queue: readonly SharedQueued[];
  readonly commands: readonly ComposerCommand[];
  readonly unobserved: readonly SharedQueued[];
  readonly held: string | null;
  readonly awaiting_turn: string | null;
  readonly operations?: readonly DocumentOperation[];
}
export interface DocumentUpdate { readonly document: string; readonly revision: number; readonly update: DocumentBytes }
export type DocumentTextInput = { readonly kind: "update"; readonly number: number; readonly document: string; readonly update: DocumentBytes };
export type DocumentMutationInput = { readonly kind: "mutate"; readonly number: number; readonly mutation: import("./composer").ComposerMutation };
export type DocumentInput = DocumentTextInput | DocumentMutationInput;
export interface DocumentOperation {
  readonly operation: string;
  readonly delivery: NoticeId | null;
  readonly accepted: boolean;
  readonly landed: import("./bridge").Landed | null;
  readonly failure: string | null;
}
export interface DocumentFailure {
  readonly message: string;
  readonly code?: string;
  readonly denial?: unknown;
  readonly not_run?: boolean;
  readonly capacity?: { readonly reason: "settling"; readonly unfinished: number }
    | { readonly reason: "pending" | "expired" } | { readonly reason: "waiting"; readonly seconds: number };
}
export type DocumentFrame =
  | { readonly kind: "result"; readonly number: number; readonly revision: number; readonly operation: DocumentOperation }
  | { readonly kind: "state"; readonly version: 1; readonly state: SharedComposer; readonly documents: readonly DocumentUpdate[]; readonly vectors?: readonly DocumentVector[]; readonly mutations?: boolean }
  | { readonly kind: "update"; readonly revision: number; readonly document: DocumentUpdate }
  | { readonly kind: "saved"; readonly number: number; readonly document: string; readonly revision: number; readonly document_revision: number }
  | { readonly kind: "refused"; readonly number: number; readonly failure: DocumentFailure };
