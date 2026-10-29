// Files the conversation points at, wherever they are: a small picture of
// one made on its machine when it is shown, the whole of it fetched to this
// device when it is opened, and opening or showing a file on this device
// (`src-tauri/src/local_files.rs`).

import { invoke } from "@tauri-apps/api/core";
import type { AttachmentSource } from "./attachments";
import type { MachineName } from "./events";
import type { HeldRead } from "./reads";

export type { AttachmentSource } from "./attachments";

/** A picture made small on the machine holding it, never kept there. */
export interface Thumbnail {
  content?: import("./events").Fingerprint;
  width: number;
  height: number;
  mime: string;
  base64: string;
}

/** The picture as an address an `<img>` takes. */
export const thumbnailUrl = (thumbnail: Thumbnail): string => `data:${thumbnail.mime};base64,${thumbnail.base64}`;

/** A file on a machine, made at most `max` pixels on its longer side.
 *  `machine` null is this computer. */
export function thumbnail(machine: MachineName | null, path: string, max: number): Promise<Thumbnail> {
  return invoke("thumbnail", { machineName: machine, path, max });
}

/** A file the user sent that could not be had; `gone` when neither the
 *  World's copy nor the host's own record holds it any more. */
export class AttachmentError extends Error {
  constructor(
    message: string,
    readonly gone: boolean,
  ) {
    super(message);
  }
}

const typed = (error: unknown): never => {
  if (error && typeof error === "object" && "kind" in error && "message" in error) {
    throw new AttachmentError(String(error.message), error.kind === "original_gone");
  }
  throw error instanceof Error ? error : new Error(String(error));
};

/** A file the user sent, made small by the machine keeping it; an agent
 *  assisting the user may show those it was sent. */
export function attachmentThumbnail(attachment: AttachmentSource, max: number): Promise<Thumbnail> {
  return invoke<Thumbnail>("thumbnail_attachment", { attachment, max }).catch(typed);
}

/** A file the user sent, brought whole to this device: from the World's
 *  copy, or where that is gone, from its host's own record. */
export interface FetchedAttachment {
  path: string;
  name: string;
  media_type: string;
  source: "original" | "host_record";
}

export function fetchAttachment(attachment: AttachmentSource, held?: HeldRead): Promise<FetchedAttachment> {
  return invoke<FetchedAttachment>("fetch_attachment", { attachment, ...held?.args }).catch(typed);
}

/** Saves a copy of a file on this device where the user picks, through the
 *  system's own dialog. */
export type Saved = { kind: "saved" } | { kind: "cancelled" } | { kind: "failed"; reason: string };

export function saveAs(path: string, suggestedName: string): Promise<Saved> {
  return invoke("save_as", { path, suggestedName });
}

/** Brings a file to this device and says where it now is; one on this
 *  computer is where it already was. `held` lets go of it and hears how
 *  it goes. */
export function fetchFile(machine: MachineName | null, path: string, held?: HeldRead): Promise<string> {
  return invoke("fetch_file", { machineName: machine, path, ...held?.args });
}

/** What opening a file on this device came to: it opened, or it is a
 *  program, which runs when opened, and was left for the user to choose. */
export type Opened = { kind: "opened" } | { kind: "program" };

/** Opens a file on this device with the program the system keeps for it; a
 *  program only when `run` says the user chose to run it. */
export function openLocal(path: string, run = false): Promise<Opened> {
  return invoke("open_local", { path, run });
}

/** Shows a file on this computer in its folder. */
export function reveal(path: string): Promise<void> {
  return invoke("reveal", { path });
}

/** On a Mac, System Settings where Fastctx is let into what macOS keeps
 *  private, with the World program shown beside it to put on the list. */
export function allowPrivacy(): Promise<void> {
  return invoke("allow_privacy");
}
