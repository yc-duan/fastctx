// An agent's words in the page as they grow. The text is read whole each
// time, as a whole text reads, and cut into its top-level blocks; a block
// whose source is unchanged keeps its nodes, so words streaming in at any
// rate redraw only the paragraph, list or block of code they are adding
// to, never what was said before it.

import { proseBlocks } from "./prose";

interface Part {
  key: string;
  nodes: ChildNode[];
}

export class StreamedProse {
  readonly element: HTMLElement;
  private parts: Part[] = [];
  private text: string | null = null;
  private writing = false;

  constructor(className = "prose") {
    this.element = document.createElement("div");
    this.element.className = className;
  }

  /** Brings the words up to `text`; `writing` while more may come. Says
   *  whether anything was drawn. */
  update(text: string, writing: boolean): boolean {
    if (text === this.text && writing === this.writing) return false;
    this.text = text;
    this.writing = writing;
    this.element.classList.toggle("writing", writing);
    const blocks = proseBlocks(text, writing);
    const next: Part[] = [];
    blocks.forEach((block, index) => {
      // The block being written is told apart from the same block done:
      // done, it is drawn once more, its code coloured.
      const key = writing && index === blocks.length - 1 ? `${block.source}\u0000w` : block.source;
      const old = this.parts[index];
      if (old?.key === key) {
        next.push(old);
        return;
      }
      const holder = document.createElement("template");
      holder.innerHTML = block.html();
      const nodes = [...holder.content.childNodes];
      // The new nodes stand where the part replaced stood: before the first
      // node of any part after it.
      const anchor = this.parts.slice(index + 1).find((part) => part.nodes.length)?.nodes[0] ?? null;
      for (const node of old?.nodes ?? []) node.remove();
      this.element.insertBefore(holder.content, anchor);
      next.push({ key, nodes });
    });
    for (const gone of this.parts.slice(blocks.length)) for (const node of gone.nodes) node.remove();
    this.parts = next;
    return true;
  }
}
