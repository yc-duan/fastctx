// The settings' one line of this device's updates (`world-gui.md`, 设置): the
// version of Fastctx here, and beside it the one thing to know, a new
// version, an update under way or gone wrong, or the World's program here
// other than the interface's; 「更新…」 opens the card where all of it is
// done (`update-card.ts`).

import { escape } from "../conversation/prose";
import { buildOrder, sameBuild } from "../world/renewal";
import type { Renewing } from "./renewal";
import { effect } from "./signals";
import { lineOf, stageMark } from "./update-marks";
import type { Updates } from "./updates";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdateWords } from "./update-words";
import type { SettingsWords } from "./words";

export class UpdateLine {
  private readonly stop: () => void;
  /** What the World's program here is beside the interface's, asked once. */
  private service = "";

  constructor(
    private readonly element: HTMLElement,
    private readonly updates: Updates,
    private readonly words: UpdateWords,
    private readonly view: UpdateViewWords,
    settings: SettingsWords,
    renewing: Renewing | null,
    open: () => void,
  ) {
    element.hidden = false;
    element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-updates-open]")) open();
    });
    this.stop = effect(() => {
      updates.offered.get();
      this.draw(updates.state.get());
    });
    void renewing?.state().then(
      (status) => {
        const node = status.node?.build ?? null;
        if (status.mismatch) {
          const legacy = { version: status.mismatch.node_version, protocol: status.mismatch.node_protocol, digest: "?" };
          this.service = settings.versionDifferent(buildOrder(status.client, legacy));
        } else if (node && !sameBuild(status.client, node)) this.service = settings.versionDifferent(buildOrder(status.client, node));
        this.draw(updates.state.peek());
      },
      () => {},
    );
  }

  leave(): void {
    this.stop();
  }

  private draw(state: import("../world/updates").UpdateState | null): void {
    const { words } = this;
    const offered = this.updates.offered.peek();
    const stage = state?.stage ? stageMark(state.stage, lineOf(this.updates.version), this.view) : null;
    const [said, tone] = stage ? [stage.said, stage.kind === "failed" ? " settings-wrong" : ""] : offered ? [words.available(offered.manifest.version), ""] : this.service ? [this.service, " settings-warn"] : ["", ""];
    const html = `<dt>${escape(words.updates)}</dt><dd><span>${escape(words.current(this.updates.version))}</span>${said ? `<span class="update-line-said${tone}">${escape(said)}</span>` : ""}<button class="text-button quiet-button" type="button" data-updates-open>${escape(words.open)}</button></dd>`;
    if (this.element.innerHTML !== html) this.element.innerHTML = html;
  }
}
