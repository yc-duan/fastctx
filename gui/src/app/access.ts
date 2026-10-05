// What makes the interface usable from the keyboard and by a screen
// reader everywhere at once, rather than page by page: a group of choices
// (`role="radiogroup"`) and a row of tabs (`role="tablist"`) are moved
// through with the arrow keys, Home and End, the one reached being chosen,
// as such controls behave elsewhere; a sheet that opens is named by its
// title.

import { redrawKeepingFocus } from "./focus";

const NEXT = new Set(["ArrowRight", "ArrowDown"]);
const PREVIOUS = new Set(["ArrowLeft", "ArrowUp"]);

export function installAccess(): void {
  document.addEventListener("keydown", (event) => {
    const target = event.target as HTMLElement | null;
    if (!target || event.altKey || event.ctrlKey || event.metaKey) return;
    const role = target.getAttribute("role");
    const group = role === "radio" ? target.closest('[role="radiogroup"]') : role === "tab" ? target.closest('[role="tablist"]') : null;
    if (!group || !(NEXT.has(event.key) || PREVIOUS.has(event.key) || event.key === "Home" || event.key === "End")) return;
    const members = [...group.querySelectorAll<HTMLElement>(`[role="${role}"]`)].filter(
      (member) => !(member as HTMLButtonElement).disabled && member.getAttribute("aria-disabled") !== "true",
    );
    if (!members.length) return;
    const at = members.indexOf(target);
    const next =
      event.key === "Home"
        ? members[0]!
        : event.key === "End"
          ? members[members.length - 1]!
          : members[(at + (NEXT.has(event.key) ? 1 : members.length - 1)) % members.length]!;
    event.preventDefault();
    next.focus();
    // Choosing may draw the group again; focus stays on the one chosen.
    redrawKeepingFocus(document.body, () => next.click());
  });

  // A sheet is named by its title, whichever page made it, and again when
  // the page draws the sheet anew while it is open.
  new MutationObserver((changes) => {
    for (const change of changes) {
      const dialog = change.target instanceof Element ? change.target.closest("dialog") : null;
      if (dialog?.open) name(dialog);
    }
  }).observe(document.body, { attributes: true, attributeFilter: ["open"], childList: true, subtree: true });
}

let named = 0;

function name(dialog: HTMLDialogElement): void {
  if (dialog.hasAttribute("aria-label")) return;
  const title = dialog.querySelector<HTMLElement>(".sheet-title");
  if (!title) return;
  title.id ||= `sheet-title-${++named}`;
  if (dialog.getAttribute("aria-labelledby") !== title.id) dialog.setAttribute("aria-labelledby", title.id);
}
