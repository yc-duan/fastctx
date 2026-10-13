// A file's bytes read for the user from the machine that has it, as the
// core describes the read (`fastctx-types/src/file_read.rs`). The shell
// takes the stream; the page sees what was read, from where, and how the
// read ended.

import type { MachineName, Timestamp } from "./wire";

export type FileRange = { kind: "all" } | { kind: "tail"; bytes: number } | { kind: "range"; offset: number; bytes: number };

export interface FileMetadata {
  size: number;
  modified: Timestamp;
}

/** The file as it was when opened, and the exact bytes chosen of it. */
export interface FileReadInfo {
  machine: MachineName;
  path: string;
  file: FileMetadata;
  offset: number;
  length: number;
  /** Which bytes these are; a read that broke off goes on from where it
   *  was while it stays the same. Absent from older nodes. */
  version?: string;
}

export interface FileReadComplete {
  bytes: number;
  file: FileMetadata;
}

export type FileReadErrorKind = "not_found" | "permission_denied" | "is_directory" | "not_regular_file" | "invalid_input" | "changed" | "io";

/** A read that did not end well; the bytes already given are not to be
 *  trusted as the file. */
export interface FileReadError {
  kind: FileReadErrorKind;
  message: string;
  bytes: number;
  observed: FileMetadata | null;
}

export type FileReadResult = { Ok: FileReadComplete } | { Err: FileReadError };

export type FileReadPart = { kind: "opened"; info: FileReadInfo } | { kind: "chunk"; offset: number; bytes: number };
