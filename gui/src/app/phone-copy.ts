// How much of the World this phone keeps on itself (design §28.39): its
// copy follows each network while the app runs in the background, and the
// pages read it first, so a conversation reads at once and while the
// network is away. Recent, larger files taken as they are opened, by
// default; or everything. Chosen here, held so at once.

import { invoke } from "@tauri-apps/api/core";
import { escape } from "../conversation/prose";
import type { SettingsWords } from "./words";

export type CopyTier = "light" | "whole";

export interface CopySeam {
  tier(): Promise<CopyTier>;
  setTier(tier: CopyTier): Promise<unknown>;
}

const call = <T>(command: string, args?: Record<string, unknown>) => invoke<T>(`plugin:android-world|${command}`, args);

export const copySeam: CopySeam = {
  tier: () => call<CopyTier>("copy_tier"),
  setTier: (tier) => call("copy_tier_set", { tier }),
};

/** The settings row that says how much the phone keeps, and changes it. */
export class CopyRow {
  private tier: CopyTier | null = null;
  private said = "";

  constructor(
    private readonly element: HTMLElement,
    private readonly words: SettingsWords,
    private readonly copy: CopySeam,
  ) {
    element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-copy]");
      const tier = button?.dataset.copy as CopyTier | undefined;
      if (!tier || tier === this.tier) return;
      void this.copy.setTier(tier).then(
        () => {
          this.tier = tier;
          this.said = "";
          this.draw();
        },
        (error: unknown) => {
          this.said = this.words.notDone(String(error instanceof Error ? error.message : error));
          this.draw();
        },
      );
    });
    void this.copy.tier().then(
      (tier) => {
        this.tier = tier;
        this.draw();
      },
      () => {
        this.element.hidden = true;
      },
    );
  }

  private draw(): void {
    const { words } = this;
    const choice = (tier: CopyTier, label: string) =>
      `<button class="text-button quiet-button" type="button" data-copy="${tier}" aria-pressed="${this.tier === tier}">${escape(label)}</button>`;
    this.element.hidden = this.tier === null;
    this.element.innerHTML = `<dt>${escape(words.phoneCopy)}</dt><dd>${choice("light", words.phoneCopyLight)}${choice("whole", words.phoneCopyWhole)}<p class="settings-said">${escape(
      this.tier === "whole" ? words.phoneCopyWholeSaid : words.phoneCopyLightSaid,
    )}</p>${this.said ? `<p class="settings-said" role="status">${escape(this.said)}</p>` : ""}</dd>`;
  }
}
