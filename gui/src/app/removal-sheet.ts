// Taking fastctx off this computer as a dialog of its own, over whatever
// page opened it: the settings, a managed machine's one page, and joining
// as one, which starts from a Fastctx never used (`world-network.md`,
// 受管机器). Made the first time it is opened, and kept.

import { Sheet } from "./confirm";
import { RemovalSection, type Removing } from "./removal";
import type { RemovalWords } from "./removal-words";

export class RemovalSheet {
  private sheet: Sheet | null = null;
  private section: RemovalSection | null = null;

  constructor(
    private readonly words: RemovalWords,
    private readonly removing: Removing,
  ) {}

  /** Opens it; on one of the two ways when `everything` says which. */
  open(everything?: boolean): void {
    if (!this.sheet) {
      const opened = new Sheet("removal-sheet");
      opened.body.innerHTML = `${opened.head(this.words.title, this.words.cancel)}<div class="removal"></div>`;
      this.section = new RemovalSection(opened.body.querySelector<HTMLElement>(".removal")!, this.words, this.removing, (on) => (opened.busy = on));
      this.sheet = opened;
    }
    this.sheet.show();
    if (everything !== undefined) this.section?.choose(everything);
  }
}
