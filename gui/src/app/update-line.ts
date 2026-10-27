// The settings' one line of this device's updates (`world-gui.md`, 设置): the
// version of Fastctx here, and beside it the one thing to know, a new
// version, an update under way or gone wrong, or the World's program here
// other than the interface's; 「更新…」 opens the card where all of it is
// done (`update-card.ts`), and where this device is stuck, 「修复」 starts a
// repair there.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { buildOrder, sameBuild } from "../world/renewal";
import type { Renewing } from "./renewal";
import { effect } from "./signals";
import { lineOf, shortVersion, stageMark } from "./update-marks";
import { troubled, type Updates } from "./updates";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdateWords } from "./update-words";
import type { SettingsWords } from "./words";

/** How often the World's program here is asked about while the page is open. */
const SERVICE_MS = 30_000;

export class UpdateLine {
  private readonly stop: () => void;
  /** What the World's program here is beside the interface's. */
  private service = "";
  private asking = 0;

  constructor(
    private readonly element: HTMLElement,
    private readonly updates: Updates,
    private readonly words: UpdateWords,
    private readonly view: UpdateViewWords,
    private readonly settings: SettingsWords,
    renewing: Renewing | null,
    open: () => void,
  ) {
    element.hidden = false;
    element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-updates-open]")) open();
      if ((event.target as HTMLElement).closest("[data-updates-repair]")) {
        updates.repair();
        open();
      }
    });
    this.stop = effect(() => {
      updates.offered.get();
      this.draw(updates.state.get());
    });
    // How this computer's service stands beside the interface, asked again
    // while the page is open: a change to the new program ends by itself.
    const ask = (): void => {
      void renewing?.state().then(
        (status) => {
          const node = status.node?.build ?? null;
          if (status.mismatch) {
            const legacy = { version: status.mismatch.node_version, protocol: status.mismatch.node_protocol, digest: "?" };
            this.service = settings.versionDifferent(buildOrder(status.client, legacy));
          } else if (node && !sameBuild(status.client, node)) this.service = settings.versionDifferent(buildOrder(status.client, node));
          else this.service = "";
          this.draw(updates.state.peek());
        },
        () => {},
      );
    };
    ask();
    this.asking = window.setInterval(ask, SERVICE_MS);
  }

  leave(): void {
    this.stop();
    clearInterval(this.asking);
  }

  private draw(state: import("../world/updates").UpdateState | null): void {
    const { words } = this;
    const offered = this.updates.offered.peek();
    const line = lineOf(this.updates.version);
    const stage = state?.stage ? stageMark(state.stage, line, this.view) : null;
    // The one thing to know, as the machines' update view marks it: what is
    // under way or went wrong, else a new version; the sentence behind it.
    const mark = stage
      ? { kind: stage.kind, html: stage.html, said: [stage.said, stage.why].filter(Boolean).join("\n") }
      : offered
        ? {
            kind: "behind",
            html: `${glyph("update", 16)}<span class="ml-num">${escape(shortVersion(offered.manifest.version, line))}</span>`,
            said: words.available(offered.manifest.version),
          }
        : null;
    const beside = mark
      ? `<span class="uv-mark ${mark.kind}" title="${escape(mark.said)}">${mark.html}</span>`
      : this.service
        ? `<span class="settings-said settings-warn">${escape(this.service)}</span>`
        : "";
    const repair = state?.repair_supported && troubled(state, offered) ? `<button class="text-button quiet-button" type="button" data-updates-repair>${escape(words.repair.repair)}</button>` : "";
    const html = `<dt>${escape(this.settings.version)}</dt><dd><span class="sd-version">${escape(this.updates.version)}</span>${beside}</dd><dd class="sd-act">${repair}<button class="text-button quiet-button" type="button" data-updates-open>${escape(words.open)}</button></dd>`;
    if (this.element.innerHTML !== html) this.element.innerHTML = html;
  }
}
