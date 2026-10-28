import type { Attachment } from "./attachments";
import type { ComposerCommand, ComposerDraft } from "./composer";
import type { MachineName, NoticeId } from "./wire";

/** Yjs v1 bytes in JSON; the document binding decodes them to Uint8Array. */
export type DocumentBytes = string;
export interface DocumentSend {
  readonly operation: string;
  readonly do: "send_document";
  readonly document: string;
  readonly snapshot: DocumentBytes;
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
}
export interface DocumentUpdate { readonly document: string; readonly revision: number; readonly update: DocumentBytes }
export type DocumentInput = { readonly kind: "update"; readonly number: number; readonly document: string; readonly update: DocumentBytes };
export interface DocumentFailure {
  readonly message: string;
  readonly code?: string;
  readonly denial?: unknown;
  readonly not_run?: boolean;
  readonly capacity?: { readonly reason: "settling"; readonly unfinished: number }
    | { readonly reason: "pending" | "expired" } | { readonly reason: "waiting"; readonly seconds: number };
}
export type DocumentFrame =
  | { readonly kind: "state"; readonly version: 1; readonly state: SharedComposer; readonly documents: readonly DocumentUpdate[] }
  | { readonly kind: "update"; readonly revision: number; readonly document: DocumentUpdate }
  | { readonly kind: "saved"; readonly number: number; readonly document: string; readonly revision: number; readonly document_revision: number }
  | { readonly kind: "refused"; readonly number: number; readonly failure: DocumentFailure };
