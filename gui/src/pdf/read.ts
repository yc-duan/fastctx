import { invoke } from "@tauri-apps/api/core";
import type { FileReadInfo } from "../world/file-read";
import type { HeldRead } from "../world/reads";

export interface PdfRead {
  info: FileReadInfo;
  /** A view into the IPC buffer. PdfView.load takes ownership of the buffer. */
  bytes: Uint8Array;
}

/** Reads once through the World, with the metadata and bytes from the same
 * stream. The existing base64 readBytes remains suitable for image data URLs. */
export async function readPdf(machine: string, path: string, held?: HeldRead): Promise<PdfRead> {
  const response = await invoke<ArrayBuffer>("read_binary", { machineName: machine, path, ...held?.args });
  if (!(response instanceof ArrayBuffer) || response.byteLength < 4) throw new Error("Invalid binary file response");
  const length = new DataView(response).getUint32(0, true);
  const start = 4 + length;
  if (start > response.byteLength) throw new Error("Incomplete binary file metadata");
  const info: FileReadInfo = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(response, 4, length)));
  const bytes = new Uint8Array(response, start);
  if (info.length !== bytes.byteLength) throw new Error("Incomplete binary file content");
  return { info, bytes };
}
