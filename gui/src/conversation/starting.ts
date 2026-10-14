// A conversation being begun (`world-gui.md`, 界面的结构): the moment the
// first words are sent they rise from the input into the conversation, and
// beneath them the turn's line says it is starting, until the agent begins
// and its first word arrives, in the conversation's own page, which opens
// in the same state. What happens on the way (a place made for it on its
// machine, its directory trusted, the agent woken, the words handed over)
// is not shown; if any of it fails, the reason is said right there, under
// the words.

import type { Words } from "../words";
import type { ShownAttachment } from "./model";
import { Conversation } from "./conversation";
import { ConversationView } from "./view";
import type { Departure } from "./flight";

/** The first words, carried to the conversation's page as they were shown. */
export interface Seed {
  text: string;
  attached: ShownAttachment[];
}

export class Starting {
  readonly element: HTMLElement;
  private conversation: Conversation;
  private view: ConversationView;
  private seeded: Seed | null = null;

  constructor(
    private readonly words: Words,
    private readonly here: () => string,
    private readonly departure: () => Departure | null,
  ) {
    this.element = document.createElement("div");
    this.element.className = "talk-scroller scroller starting-scroller";
    this.element.hidden = true;
    const root = document.createElement("div");
    root.className = "conversation";
    this.element.append(root);
    this.conversation = new Conversation();
    this.view = this.made(root);
  }

  private made(root: HTMLElement): ConversationView {
    return new ConversationView(root, this.conversation, {
      words: this.words,
      english: this.words.english,
      here: this.here(),
      residentMachine: this.here(),
      scroller: this.element,
      reading: () => {},
      departure: this.departure,
    });
  }

  /** The first words go: shown at once, rising from the input, the line
   *  beneath them starting. */
  begin(seed: Seed): void {
    this.clear();
    this.seeded = seed;
    this.element.hidden = false;
    // Drawn once empty, so the words come in as they do in any
    // conversation already there.
    this.view.update();
    this.conversation.sending(seed.text, this.here(), seed.attached);
    requestAnimationFrame(() => this.view.update());
  }

  /** It could not begin: why, as it was given, under the words. */
  failed(why: string): void {
    this.conversation.failedToStart(why);
    this.view.update();
  }

  /** The words being carried to the conversation's page. */
  seed(): Seed | null {
    return this.seeded;
  }

  /** Empty again, for the next conversation begun here. */
  clear(): void {
    this.seeded = null;
    this.element.hidden = true;
    this.view.destroy();
    this.conversation = new Conversation();
    const root = this.element.querySelector<HTMLElement>(".conversation")!;
    const fresh = document.createElement("div");
    fresh.className = "conversation";
    root.replaceWith(fresh);
    this.view = this.made(fresh);
  }
}
