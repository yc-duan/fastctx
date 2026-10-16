// The network's update policy, its super administrator's to set
// (`world-upgrade.md`, 谁来更新): the version it asks each machine to run
// at least, picked from the window of versions and said to the machines
// below it, never enforced; and whether its managed machines take new
// releases by themselves. Which line a device follows is the line of the
// version it runs: the network sets none. Devices of 1.0.1 beta 1 and 2 still
// follow the line a hub keeps, so the policy names the test releases' line
// for them, as a later hub tells them whatever it keeps.

import { Sheet } from "./confirm";
import { escape } from "../conversation/prose";
import type { Policy } from "../world/updates";
import type { UpdateWords } from "./update-words";

export type { Policy };

export class UpdatePolicySheet {
  private sheet: Sheet | null = null;
  private minimum: string | null = null;
  private managedAuto = true;

  constructor(
    private readonly words: UpdateWords,
    private readonly save: (policy: Policy) => Promise<void>,
    /** Opens the window of versions to pick the lowest one, or none. */
    private readonly versions: (current: string | null, pick: (version: string | null) => void) => void,
  ) {}

  open(current: Policy): void {
    this.minimum = current.minimum ?? null;
    this.managedAuto = current.managed_auto ?? true;
    const sheet = this.sheet ?? this.make();
    this.draw();
    sheet.show();
  }

  private make(): Sheet {
    const sheet = new Sheet("update-policy-sheet");
    this.sheet = sheet;
    sheet.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const choice = target.closest<HTMLButtonElement>("[data-policy-managed]");
      if (choice) {
        this.managedAuto = choice.dataset.policyManaged === "on";
        this.draw();
      }
      if (target.closest("[data-policy-minimum]")) {
        this.versions(this.minimum, (version) => {
          this.minimum = version;
          this.draw();
        });
      }
      if (target.closest("[data-policy-save]")) this.submit(sheet);
    });
    return sheet;
  }

  private draw(): void {
    const { words, sheet } = this;
    if (!sheet) return;
    const choice = (on: boolean, name: string) =>
      `<button type="button" class="choice${on === this.managedAuto ? " chosen" : ""}" role="radio" aria-checked="${on === this.managedAuto}" data-policy-managed="${on ? "on" : "off"}">${escape(name)}</button>`;
    sheet.body.innerHTML = `${sheet.head(words.policy, words.close)}
      <div class="policy-rows">
        <div class="choice-row"><span class="choice-label">${escape(words.minimumField)}</span><div class="policy-minimum"><span class="policy-minimum-now">${escape(this.minimum ?? words.minimumNone)}</span><button class="text-button quiet-button" type="button" data-policy-minimum>${escape(words.minimumPick)}</button></div></div>
        <div class="choice-row" role="radiogroup" aria-label="${escape(words.managedAuto)}"><span class="choice-label">${escape(words.managedAuto)}</span><div class="choice-options">${choice(true, words.on)}${choice(false, words.off)}</div></div>
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <div class="removal-go"><button class="sheet-go" type="button" data-policy-save>${escape(words.save)}</button></div>`;
  }

  private submit(sheet: Sheet): void {
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    sheet.busy = true;
    this.save({ channel: "beta", ...(this.minimum ? { minimum: this.minimum } : {}), managed_auto: this.managedAuto }).then(
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
