// A new password and its repeat, the same wherever one is set
// (`world-gui.md`, 账号、机器、共享与协助): before anything is typed, the bar it
// must clear is said quietly under it; as it is typed, what it still lacks,
// then how strong it is in three marks; what is wrong when the form is sent
// is said there too, beside the fields, not at the foot of the form.

import { escape } from "../conversation/prose";
import { passwordProblem, passwordStrength, type PasswordProblem } from "./password";

export interface NewPasswordWords {
  password: string;
  passwordAgain: string;
  /** The bar a password must clear, said before it is typed. */
  passwordFloor: string;
  passwordRule: Record<PasswordProblem, string>;
  passwordsDiffer: string;
  /** Enough, good, strong. */
  strength: readonly [string, string, string];
}

/** The two fields, named `password` and `again`, side by side or one under
 *  the other, and the line under them. They are not left to the browser's
 *  own checks, which would speak first in a box of the system's: an empty
 *  one is said on the line under them, as anything else wrong is. */
export function newPasswordFields(words: NewPasswordWords, { pair = true, password = "", again = "" } = {}): string {
  const fields = `<label class="field"><span class="field-label">${escape(words.password)}</span><input name="password" type="password" autocomplete="new-password" value="${escape(password)}"></label>
    <label class="field"><span class="field-label">${escape(words.passwordAgain)}</span><input name="again" type="password" autocomplete="new-password" value="${escape(again)}"></label>`;
  return `${pair ? `<div class="field-pair">${fields}</div>` : fields}
    <p class="password-meter" aria-live="polite"><span class="password-bars"><i></i><i></i><i></i></span><span class="password-said">${escape(words.passwordFloor)}</span></p>`;
}

/** The line under the fields, as they now stand: the bar while nothing is
 *  typed, what it lacks, how strong it is. */
export function sayPassword(root: ParentNode, words: NewPasswordWords, account: string): void {
  const meter = root.querySelector<HTMLElement>(".password-meter");
  if (!meter) return;
  const password = root.querySelector<HTMLInputElement>("[name=password]")?.value ?? "";
  const problem = password ? passwordProblem(account, password) : null;
  const strength = passwordStrength(account, password);
  meter.dataset.strength = String(strength);
  meter.classList.toggle("wanting", problem !== null);
  meter.querySelector(".password-said")!.textContent = !password ? words.passwordFloor : problem ? words.passwordRule[problem] : words.strength[strength - 1]!;
}

/** Whether the password may be sent; if not, what is wrong is said on the
 *  line under the fields and the field to change is focused. */
export function passwordReady(root: ParentNode, words: NewPasswordWords, account: string): boolean {
  const password = root.querySelector<HTMLInputElement>("[name=password]");
  const again = root.querySelector<HTMLInputElement>("[name=again]");
  const meter = root.querySelector<HTMLElement>(".password-meter");
  const value = password?.value ?? "";
  const problem = passwordProblem(account, value);
  const said = problem ? words.passwordRule[problem] : value !== (again?.value ?? "") ? words.passwordsDiffer : null;
  if (!said) return true;
  if (meter) {
    meter.classList.add("wanting");
    meter.querySelector(".password-said")!.textContent = said;
  }
  (problem ? password : again)?.focus();
  return false;
}
