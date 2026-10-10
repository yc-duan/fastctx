// Shown once, at start, when a terminal or Codex finds another fastctx first
// on its PATH: typing `fastctx` there does not reach this computer's World,
// and an old release's unapply would delete the whole .fastctx. The settings
// row keeps saying it afterwards.

import { escape } from "../conversation/prose";
import type { RenewalStatus } from "../world/renewal";
import { Sheet } from "./confirm";
import { pathFacts, pathSignature } from "./path-facts";
import type { SettingsWords } from "./words";

const SEEN = "path-notice";

export function showPathNotice(status: RenewalStatus, words: SettingsWords, close: string): void {
  const facts = pathFacts(status.commands, words);
  if (!facts.length) return;
  const signature = pathSignature(facts);
  try {
    if (localStorage.getItem(SEEN) === signature) return;
  } catch {
    // Without storage the notice is shown each time; nothing else depends on it.
  }
  const sheet = new Sheet("confirm-sheet path-sheet");
  const rows = facts
    .map((fact) => `<li class="path-row" title="${escape(fact.detail)}"><span>${escape(fact.text)}</span><code>${escape(fact.detail.split("\n")[0] ?? "")}</code></li>`)
    .join("");
  const legacy = facts.some((fact) => fact.legacy) ? `<p class="choice-margin choice-warn">${escape(words.pathLegacy)}</p>` : "";
  sheet.body.innerHTML = `${sheet.head(words.pathTitle, close)}<ul class="path-rows">${rows}</ul>${legacy}<footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.pathOk)}</button></footer>`;
  sheet.dialog.addEventListener("close", () => {
    try {
      localStorage.setItem(SEEN, signature);
    } catch {
      // As above.
    }
  });
  sheet.show();
}
