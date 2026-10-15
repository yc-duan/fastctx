// The network's update policy, its super administrator's to set
// (`world-upgrade.md`, 谁来更新): the channel its devices follow unless their
// owners choose, and the version it asks each machine to run at least,
// which is said to the machines below it, never enforced.

import { Sheet } from "./confirm";
import { escape } from "../conversation/prose";
import type { Channel } from "../world/updates";
import type { UpdateWords } from "./update-words";

export interface Policy {
  channel: Channel;
  minimum?: string;
}

export class UpdatePolicySheet {
  private sheet: Sheet | null = null;
  private channel: Channel = "stable";

  constructor(
    private readonly words: UpdateWords,
    private readonly save: (policy: Policy) => Promise<void>,
  ) {}

  open(current: Policy): void {
    const { words } = this;
    const sheet = this.sheet ?? new Sheet("update-policy-sheet");
    if (!this.sheet) {
      this.sheet = sheet;
      sheet.body.addEventListener("click", (event) => {
        const target = event.target as HTMLElement;
        const choice = target.closest<HTMLButtonElement>("[data-policy-channel]");
        if (choice) {
          this.channel = choice.dataset.policyChannel as Channel;
          for (const button of sheet.body.querySelectorAll<HTMLButtonElement>("[data-policy-channel]")) {
            const chosen = button.dataset.policyChannel === this.channel;
            button.classList.toggle("chosen", chosen);
            button.setAttribute("aria-checked", String(chosen));
          }
        }
        if (target.closest("[data-policy-save]")) this.submit(sheet);
      });
    }
    this.channel = current.channel;
    const choice = (value: Channel, name: string) =>
      `<button type="button" class="choice${value === this.channel ? " chosen" : ""}" role="radio" aria-checked="${value === this.channel}" data-policy-channel="${value}">${escape(name)}</button>`;
    sheet.body.innerHTML = `${sheet.head(words.policy, words.close)}
      <div class="policy-rows">
        <div class="choice-row" role="radiogroup" aria-label="${escape(words.channel)}"><span class="choice-label">${escape(words.channel)}</span><div class="choice-options">${choice("stable", words.stable)}${choice("beta", words.beta)}</div></div>
        <label class="choice-row"><span class="choice-label">${escape(words.minimumField)}</span><input class="policy-minimum" type="text" spellcheck="false" autocomplete="off" placeholder="${escape(words.minimumNone)}" value="${escape(current.minimum ?? "")}"></label>
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <div class="removal-go"><button class="sheet-go" type="button" data-policy-save>${escape(words.save)}</button></div>`;
    sheet.show();
  }

  private submit(sheet: Sheet): void {
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    const field = sheet.body.querySelector<HTMLInputElement>(".policy-minimum")!;
    const minimum = field.value.trim().replace(/^v/, "");
    if (minimum && !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(minimum)) {
      note.hidden = false;
      note.textContent = this.words.minimumInvalid;
      field.focus();
      return;
    }
    sheet.busy = true;
    this.save({ channel: this.channel, ...(minimum ? { minimum } : {}) }).then(
      () => {
        sheet.busy = false;
        sheet.close();
      },
      (error: unknown) => {
        sheet.busy = false;
        note.hidden = false;
        note.textContent = this.words.failed(error instanceof Error ? error.message : String(error));
      },
    );
  }
}
