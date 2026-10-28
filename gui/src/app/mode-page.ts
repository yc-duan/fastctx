// The mode (`world-layers.md`): Connect or the workbench, two paper cards
// side by side, the one in use marked and the other a press away. Asked
// once when the account has not chosen; changed here from Settings after.
// The workbench's card plays its film, which tells what it adds; going back
// to Connect asks once, saying that Fastctx's conversations wait.

import { escape } from "../conversation/prose";
import { Film } from "../film/film";
import type { Built, Shape } from "../film/reel";
import { workbench, type WorkbenchWords } from "../film/workbench";
import { Sketch } from "../pen/sketch";
import type { Confirm } from "./confirm";
import { accountLayer, type LayerName } from "./layer";
import type { ModeWords } from "./mode-words";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import "./mode.css";

export interface ModeOptions {
  words: ModeWords;
  film: WorkbenchWords;
  head(title: string, actions?: string): string;
  confirm: Confirm;
  /** Where to go once a mode is chosen. */
  chosen(layer: LayerName): void;
}

export class ModePage implements Page {
  readonly element = document.createElement("section");
  readonly title: string;
  private film: Film | null = null;
  private readonly stop: () => void;
  private busy = false;

  constructor(private readonly options: ModeOptions) {
    this.title = options.words.title;
    this.element.className = "page md-page";
    this.element.addEventListener("click", (event) => this.pressed(event));
    this.stop = effect(() => {
      accountLayer.chosen.get();
      this.draw();
    });
  }

  leave(): void {
    this.stop();
    this.film?.destroy();
  }

  private draw(): void {
    const { words } = this.options;
    const chosen = accountLayer.chosen.peek();
    const card = (layer: LayerName, name: string, line: string, picture: string) => {
      const using = chosen === layer;
      const action = using
        ? `<span class="choice chosen md-using">${escape(words.using)}</span>`
        : `<button class="sheet-go md-go" type="button" data-mode="${layer}"${this.busy ? " disabled" : ""}>${escape(chosen === null ? words.choose : words.switchTo)}</button>`;
      return `<article class="md-card${using ? " using" : ""}"><div class="md-picture md-${layer}">${picture}</div><h2 class="md-name">${escape(name)}</h2><p class="md-line">${escape(line)}</p><div class="md-act">${action}</div></article>`;
    };
    this.film?.destroy();
    this.film = null;
    this.element.innerHTML = `${this.options.head(escape(chosen === null ? words.ask : words.title))}
      <div class="md-cards">
        ${card("connect", words.connect.name, words.connect.line, connectPicture())}
        ${card("workbench", words.workbench.name, words.workbench.line, "")}
      </div>`;
    const holder = this.element.querySelector(".md-workbench");
    if (holder) {
      const make = (shape: Shape): Built => workbench(this.options.film, shape);
      this.film = new Film(make, words.workbench.name, chosen === "workbench");
      holder.replaceChildren(this.film.element);
    }
  }

  private pressed(event: MouseEvent): void {
    const button = (event.target as Element | null)?.closest<HTMLButtonElement>("[data-mode]");
    if (!button || this.busy) return;
    const layer = button.dataset.mode as LayerName;
    const go = () =>
      accountLayer.choose(layer).then(() => {
        this.busy = false;
        this.options.chosen(layer);
      });
    if (layer === "connect" && accountLayer.chosen.peek() === "workbench") {
      const { words } = this.options;
      this.options.confirm.ask({ title: words.toConnect.title, what: words.toConnect.what, go: words.toConnect.go, gentle: true, act: () => go(), done: () => this.draw() });
      return;
    }
    this.busy = true;
    this.draw();
    go().catch((error: unknown) => {
      this.busy = false;
      this.draw();
      const again = this.element.querySelector<HTMLElement>(`[data-mode="${layer}"]`);
      if (again) again.title = `${this.options.words.failed}\n${detailOf(error)}`;
    });
  }
}

/** Connect, still: the user's own terminal, a stroke of ink from it to
 *  each of their machines. */
function connectPicture(): string {
  const sketch = new Sketch("mode-connect");
  sketch.wash(18, 34, 96, 66, "box", 8);
  sketch.rule([30, 52], [72, 52], 2, "graphite");
  sketch.rule([30, 64], [96, 64], 2, "graphite");
  sketch.rule([30, 76], [60, 76], 2, "graphite");
  const machines: [number, number][] = [
    [196, 18],
    [210, 64],
    [196, 110],
  ];
  for (const [x, y] of machines) {
    sketch.stroke(
      [
        [116, 67],
        [150, 67 + (y + 14 - 67) * 0.5],
        [x - 4, y + 14],
      ],
      1.4,
      "graphite",
    );
    sketch.wash(x, y, 46, 28, "field", 5);
  }
  sketch.blot([226, 32], 3, "clay");
  return sketch.svg(270, 150, "md-sketch");
}
