// Choosing a folder on a machine whose own folder picker is out of reach
// (`world-gui.md`, 智能体页): a sheet over the page with that machine's
// folders, walked a level at a time by its node (`folder-walk.ts`). The
// sheet keeps its size as the walk goes; only the list of folders scrolls.

import type { Folders } from "../world/focus";
import { Sheet } from "./confirm";
import { FolderWalk, type FolderWalkWords } from "./folder-walk";

export interface FolderSheetWords {
  chooseOn(machine: string): string;
  close: string;
  walk: FolderWalkWords;
}

export class FolderSheet {
  private readonly sheet = new Sheet("device-sheet folder-sheet");

  constructor(private readonly words: FolderSheetWords) {}

  /** Walks `machine`'s folders from its home and disks; the folder chosen
   *  goes to `chosen` as the sheet closes. */
  open(machine: string, folders: (path: string | null) => Promise<Folders>, chosen: (path: string) => void): void {
    const walk = new FolderWalk({
      words: this.words.walk,
      machine,
      folders,
      chosen: (path) => {
        this.sheet.close();
        chosen(path);
      },
    });
    this.sheet.body.innerHTML = this.sheet.head(this.words.chooseOn(machine), this.words.close);
    this.sheet.body.append(walk.element);
    this.sheet.show();
    walk.go(null);
  }
}
