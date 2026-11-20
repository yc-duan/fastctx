// The files of a conversation read in its side, each on a tab of its own
// (`world-gui.md`, 对话的侧面板): opened at what a file holds or at what the
// conversation did to it; one already open is turned to; their changes
// follow the conversation. The conversation's page reads them so, and so
// does the window its files' card stands in (`board/main.ts`), whose
// reading is asked of the desk's window.

import { split, type FileChange } from "../files/model";
import { RecentFileViews } from "../files/recent";
import { FileView, type FileLines, type ViewTab } from "../files/view";
import type { FilesWords } from "../files/words";
import type { SideParts } from "./parts";

/** How a file is read. */
export interface Reading {
  lines(machine: string, path: string): Promise<FileLines>;
  /** A picture made small by the machine holding it, as an address an
   *  `<img>` takes. */
  picture?: ((machine: string, path: string, max: number) => Promise<string>) | undefined;
  /** Opens it with this device's program for its kind. */
  outside?: ((machine: string, path: string) => void) | undefined;
  /** Whether a failure says the file is not there. */
  gone(error: unknown): boolean;
  /** A failure in words. */
  detail(error: unknown): string;
}

export interface FileTabsOptions {
  /** The side the tabs open in; none while there is none. */
  side(): SideParts | null;
  words: FilesWords;
  reading: Reading;
  /** What the conversation did to a file, the latest first. */
  changes(machine: string, path: string): FileChange[];
  /** Another file a link in one names. */
  open(machine: string, path: string): void;
  /** Where a file is shown among its machine's folders. */
  folders?: ((machine: string, path: string) => string | undefined) | undefined;
}

const keyOf = (machine: string, path: string) => `${machine}\n${path}`;

export class FileTabs {
  private readonly read = new Map<string, { machine: string; path: string; view: FileView }>();
  private readonly recent = new RecentFileViews();
  private leaving = false;

  constructor(private readonly options: FileTabsOptions) {}

  /** Turns the side to the tab a file already has, at `tab` where one is
   *  named: true where it has one. */
  turn(machine: string, path: string, tab: ViewTab | null): boolean {
    const key = keyOf(machine, path);
    const read = this.read.get(key);
    if (!read || !this.options.side()?.showTab(`file:${key}`)) return false;
    if (tab) read.view.turn(tab);
    return true;
  }

  /** Opens a file on a tab of its own, the side opened for it. `outside`:
   *  how this one opens with a program, where that is not as any file
   *  does. False where there is no side to read it in. */
  open(machine: string, path: string, tab: ViewTab | null, outside?: (machine: string, path: string) => void): boolean {
    const { options } = this;
    const side = options.side();
    if (!side || this.leaving) return false;
    const key = keyOf(machine, path);
    const { reading } = options;
    const parked = outside ? undefined : this.recent.take(key);
    const view = parked ?? new FileView({
      words: options.words,
      seams: {
        lines: reading.lines,
        picture: reading.picture,
        outside: outside ? (on, at) => (on === machine && at === path ? outside(on, at) : reading.outside?.(on, at)) : reading.outside,
        open: options.open,
        gone: reading.gone,
        detail: reading.detail,
      },
      machine,
      path,
      changes: options.changes(machine, path),
      tab: tab ?? undefined,
      now: () => Date.now(),
      own: true,
      tabbed: true,
      folders: options.folders?.(machine, path),
    });
    if (parked) {
      view.update(options.changes(machine, path));
      if (tab) view.turn(tab);
    }
    const { folder, name } = split(path);
    const opened = side.openTab({
      id: `file:${key}`,
      name,
      hint: split(folder.replace(/[\\/]+$/, "")).name,
      title: `${machine}  ${path}`,
      file: { machine, path },
      page: view,
      leave: () => {
        this.read.delete(key);
        if (this.leaving || outside) view.leave();
        else this.recent.put(key, view);
      },
    });
    if (opened) this.read.set(key, { machine, path, view });
    else view.leave();
    return opened;
  }

  /** The owner also closes the tabs that are still open. */
  leave(): void {
    this.leaving = true;
    this.recent.clear();
  }

  /** Their changes as the conversation now tells them. */
  update(): void {
    for (const read of this.read.values()) read.view.update(this.options.changes(read.machine, read.path));
  }
}
