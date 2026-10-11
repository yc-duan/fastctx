// Files the conversation points at, wherever they are: a small picture of
// one made on its machine when it is shown, the whole of it fetched to this
// device when it is opened, and opening or showing a file on this device
// (`src-tauri/src/local_files.rs`).

import { invoke } from "@tauri-apps/api/core";
import type { Attachment } from "./attachments";
import type { MachineName } from "./events";

/** A picture made small on the machine holding it, never kept there. */
export interface Thumbnail {
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

/** A file the user sent, made small by the machine keeping it; an agent
 *  lent to the user may show those it was sent. */
export function attachmentThumbnail(attachment: Attachment, max: number): Promise<Thumbnail> {
  return invoke("thumbnail_attachment", { attachment, max });
}

/** Brings a file to this device and says where it now is; one on this
 *  computer is where it already was. */
export function fetchFile(machine: MachineName | null, path: string): Promise<string> {
  return invoke("fetch_file", { machineName: machine, path });
}

/** Opens a file on this device with the program the system keeps for it. */
export function openLocal(path: string): Promise<void> {
  return invoke("open_local", { path });
}

/** Shows a file on this computer in its folder. */
export function reveal(path: string): Promise<void> {
  return invoke("reveal", { path });
}
