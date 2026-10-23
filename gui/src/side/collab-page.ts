// Who works with a conversation, as a tree drawn by hand (`world-gui.md`,
// 协作): the conversation at its root, the colleagues it asked on its
// branches, and under each the host's own agents it sent, as the
// conversation's own sent ones hang from the root. A node is its family's
// mark in a seal, its name, its task, and how it stands at its end: a
// breathing dot while it works, a tick once it handed back, a cross where it
// failed, a stroke where it was stopped; a colleague also says how its goal
// stands. How long and how many steps sit beside it; more is said when
// pointed at. Pressing one opens it.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { Sketch } from "../pen/sketch";
import type { SidePage } from "./panel";
import type { CollabNode } from "./types";
import type { CollabWords, GoalWords } from "./words";

interface Placed {
  node: CollabNode | null;
  depth: number;
}

export class CollabPage implements SidePage {
  readonly element: HTMLElement;
  private readonly tree: HTMLElement;
  private readonly chat: HTMLElement;
  /** What the chat shows, to let go of when it is left. */
  private chatting: { node: CollabNode; leave(): void } | null = null;
  private nodes: CollabNode[] = [];
  private root: { agent: "claude" | "codex"; name: string } = { agent: "claude", name: "" };
  private drawn = "";

  constructor(
    private readonly words: CollabWords,
    private readonly goals: GoalWords,
    private readonly pressed: (node: CollabNode) => void,
  ) {
    this.element = document.createElement("section");
    this.element.className = "cp-page scroller";
    this.element.innerHTML = `<div class="cp-tree"></div><div class="cp-chat" hidden></div>`;
    this.tree = this.element.querySelector(".cp-tree")!;
    this.chat = this.element.querySelector(".cp-chat")!;
    this.chat.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]")?.dataset.act;
      if (act === "back") this.back();
      else if (act === "jump") this.jumping?.();
    });
    this.tree.addEventListener("click", (event) => {
      const id = (event.target as HTMLElement).closest<HTMLElement>(".cp-node[data-id]")?.dataset.id;
      const node = id ? this.find(this.nodes, id) : null;
      if (node) this.pressed(node);
    });
    new ResizeObserver(() => this.ink()).observe(this.tree);
  }

  show(root: { agent: "claude" | "codex"; name: string }, nodes: CollabNode[]): void {
    this.root = root;
    this.nodes = nodes;
    this.draw();
  }

  shown(): void {
    this.ink();
  }

  private jumping: (() => void) | null = null;

  /** One that works with the conversation, opened in the page: a
   *  colleague's own conversation, or what one of the host's agents did,
   *  under a head that leads back to the tree. */
  open(node: CollabNode, body: HTMLElement, leave: () => void, jump: (() => void) | null): void {
    this.chatting?.leave();
    this.chatting = { node, leave };
    this.jumping = jump;
    const words = this.words;
    const tail = jump
      ? `<button class="cp-chat-act" type="button" data-act="jump">${escape(words.jump)}</button>`
      : "";
    this.chat.innerHTML = `<header class="cp-chat-head"><button class="cp-back" type="button" data-act="back" aria-label="${escape(words.back)}">${glyph("back", 16)}<span>${escape(words.back)}</span></button><span class="cp-chat-who"><span class="cp-seal">${glyph(node.agent, 14)}</span><span class="cp-name">${escape(node.name)}</span></span>${tail}</header><div class="cp-chat-body"></div>`;
    this.chat.querySelector(".cp-chat-body")!.append(body);
    this.chat.hidden = false;
    this.tree.hidden = true;
    this.element.classList.add("cp-chatting");
  }

  /** The node shown in the chat, if one is. */
  get openNode(): CollabNode | null {
    return this.chatting?.node ?? null;
  }

  back(): void {
    this.chatting?.leave();
    this.chatting = null;
    this.jumping = null;
    this.chat.hidden = true;
    this.chat.innerHTML = "";
    this.tree.hidden = false;
    this.element.classList.remove("cp-chatting");
    this.drawn = "";
    requestAnimationFrame(() => this.ink());
  }

  private draw(): void {
    const placed: Placed[] = [{ node: null, depth: 0 }];
    const walk = (list: CollabNode[], depth: number) => {
      for (const node of list) {
        placed.push({ node, depth });
        walk(node.children, depth + 1);
      }
    };
    walk(this.nodes, 1);
    const rows = placed.map((at) => (at.node ? this.row(at.node, at.depth) : this.rootRow()));
    const empty = this.nodes.length === 0 ? `<p class="cp-empty">${escape(this.words.empty)}</p>` : "";
    this.tree.innerHTML = `<div class="cp-ink" aria-hidden="true"></div>${rows.join("")}${empty}`;
    this.drawn = "";
    requestAnimationFrame(() => this.ink());
  }

  private rootRow(): string {
    return `<div class="cp-node cp-root" data-depth="0" style="--depth:0"><span class="cp-seal">${glyph(this.root.agent, 14)}</span><span class="cp-lines"><span class="cp-name">${escape(this.words.here)}</span></span></div>`;
  }

  private row(node: CollabNode, depth: number): string {
    const words = this.words;
    // A colleague says how its goal stands; one of the host's agents how
    // long it took and how many steps. The rest is said when pointed at.
    const meta = node.goal
      ? `${glyph("goal", 14)}<span>${escape(words.goal[node.goal])}</span>`
      : `<span>${escape(this.goals.worked(node.took))}</span>${node.steps !== null ? `<span>${escape(words.steps(node.steps))}</span>` : ""}`;
    const facts = [words.states[node.state], this.goals.worked(node.took), node.steps !== null ? words.steps(node.steps) : "", node.why ?? ""];
    const said = [node.name, facts.filter(Boolean).join(words.separator), node.machine ?? "", node.task].filter(Boolean).join("\n");
    const end =
      node.state === "working"
        ? '<span class="cp-pulse"></span>'
        : node.state === "done"
          ? glyph("check", 16)
          : node.state === "failed"
            ? glyph("cross", 16)
            : node.state === "stopped"
              ? '<span class="cp-stopped"></span>'
              : "";
    return `<button class="cp-node ${node.state}" type="button" data-id="${escape(node.id)}" data-depth="${depth}" style="--depth:${depth}" title="${escape(said)}"><span class="cp-seal">${glyph(node.agent, 14)}</span><span class="cp-lines"><span class="cp-top"><span class="cp-name">${escape(node.name)}</span><span class="cp-meta">${meta}</span><span class="cp-end" aria-label="${escape(words.states[node.state])}">${end}</span></span><span class="cp-task">${escape(node.task)}</span></span></button>`;
  }

  /** Each branch, from under its parent's seal down and across to the
   *  seal of what it holds, drawn when the tree's size is known. */
  private ink(): void {
    const width = this.tree.clientWidth;
    const height = this.tree.clientHeight;
    const key = `${width}x${height}`;
    if (width === 0 || key === this.drawn) return;
    this.drawn = key;
    const rows = [...this.tree.querySelectorAll<HTMLElement>(".cp-node")];
    const box = this.tree.getBoundingClientRect();
    const sketch = new Sketch("collab");
    rows.forEach((row, at) => {
      const depth = Number(row.dataset.depth);
      if (depth === 0) return;
      let parent = at - 1;
      while (parent >= 0 && Number(rows[parent]!.dataset.depth) >= depth) parent -= 1;
      if (parent < 0) return;
      const p = rows[parent]!.querySelector<HTMLElement>(".cp-seal")!.getBoundingClientRect();
      const c = row.querySelector<HTMLElement>(".cp-seal")!.getBoundingClientRect();
      const x = p.left - box.left + p.width / 2;
      const y0 = p.bottom - box.top + 3;
      const y1 = c.top - box.top + c.height / 2;
      sketch.rule([x, y0], [x, y1 - 4], 1.1, "graphite");
      sketch.rule([x, y1 - 4], [c.left - box.left - 4, y1], 1.1, "graphite");
    });
    this.tree.querySelector<HTMLElement>(".cp-ink")!.innerHTML = sketch.svg(width, height, "cp-ink-svg");
  }

  private find(list: CollabNode[], id: string): CollabNode | null {
    for (const node of list) {
      if (node.id === id) return node;
      const found = this.find(node.children, id);
      if (found) return found;
    }
    return null;
  }
}
