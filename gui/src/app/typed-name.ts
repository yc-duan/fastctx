// The last guard of a red sheet that moves or ends a whole network: the
// button that goes on waits until the network's name is typed as it is.

import { escape } from "../conversation/prose";

/** The field the network's name is typed into, under `label`. */
export function typedName(label: string): string {
  return `<label class="field move-name"><span class="field-label">${escape(label)}</span><input name="network" class="mono" autocomplete="off" spellcheck="false" autocapitalize="off" required></label>`;
}

/** Keeps the button in `form` that goes on unpressable until `name` is
 *  typed into its field. */
export function holdUntilNamed(form: HTMLElement, name: string): void {
  const go = form.querySelector<HTMLButtonElement>(".sheet-go");
  const typed = form.querySelector<HTMLInputElement>("input[name=network]");
  if (!go || !typed) return;
  const check = () => (go.disabled = typed.value.trim() !== name);
  check();
  typed.addEventListener("input", check);
}
