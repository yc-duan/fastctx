// Finding files by name for an `@` in the user's words
// (`fastctx_types::file_search`). The search lives on the machine that has
// the files, in the agent's project places there; it reads no contents.
// A conversation starts one when its composer opens, asks it on each edit,
// and closes it when it is left.

import { invoke } from "@tauri-apps/api/core";
import type { MachineName, ResidentId } from "./wire";

export type FileSearchScope = { scope: "resident"; resident: ResidentId; roots: string[] } | { scope: "roots"; roots: string[] };

export type FileSearchRequest =
  | { do: "start"; scope: FileSearchScope }
  | { do: "query"; search: string; query: string; limit: number }
  | { do: "close"; search: string };

export interface FileMatch {
  /** The file's absolute path on its machine. */
  path: string;
  /** The place it was found in, and the path within it. */
  root: string;
  relative: string;
}

export type FileSearchResult =
  | { state: "started"; search: string }
  | { state: "matches"; query: string; ready: boolean; total: number; files: FileMatch[] }
  | { state: "closed" };

export type FileSearchReply = { machine: MachineName } & FileSearchResult;

export function findFiles(machine: MachineName, request: FileSearchRequest): Promise<FileSearchReply> {
  return invoke<FileSearchReply>("find_files", { machineName: machine, request });
}

/** How many names a menu shows. */
const SHOWN = 20;

/** One conversation's search: started at once, asked on each edit, closed
 *  when the conversation is left. While the machine is still reading the
 *  names, the newest edit is asked again until it is ready; an older edit
 *  stops being asked as soon as a newer one comes. */
export class FileSearch {
  private readonly started: Promise<string>;
  private asking = 0;
  private closed = false;

  constructor(private readonly machine: MachineName, resident: ResidentId) {
    this.started = findFiles(machine, { do: "start", scope: { scope: "resident", resident, roots: [] } }).then((reply) => {
      if (reply.state !== "started") throw new Error(`${machine} answered the search's start with ${reply.state}`);
      return reply.search;
    });
    // A start the machine refused is said when the menu asks.
    this.started.catch(() => {});
  }

  async find(query: string): Promise<FileMatch[]> {
    const asked = ++this.asking;
    const search = await this.started;
    for (;;) {
      const reply = await findFiles(this.machine, { do: "query", search, query, limit: SHOWN });
      if (reply.state !== "matches") throw new Error(`${this.machine} answered a query with ${reply.state}`);
      if (reply.ready || asked !== this.asking || this.closed) return reply.files;
      // The names are still being read: as soon as a moment passes, the
      // same edit is asked again, so new names appear without typing.
      await new Promise((resolve) => setTimeout(resolve, 250));
      if (asked !== this.asking || this.closed) return reply.files;
    }
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    void this.started
      .then((search) => findFiles(this.machine, { do: "close", search }))
      .catch((error: unknown) => console.error("closing a file search", error));
  }
}
