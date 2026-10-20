// Whose own configuration a page shows (`world-user-config.md`): the account
// this device is logged in to, or where it is logged in to more than one,
// the one chosen from a pill in the head, drawn as the agents' machine pill
// is. The choice is kept on this device.

import { glyph } from "../conversation/glyphs";
import { Popover } from "../conversation/popover";
import { escape } from "../conversation/prose";
import { loginsOf, type Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { McpCall, McpReply, Profile, ProfileView } from "../world/profile";
import type { ProfileWords } from "./profile-words";

/** How the account's own pages reach its node: the World's calls, or the
 *  specimen's. */
export interface ProfileCalls {
  read(hub: string, account: string): Promise<ProfileView>;
  write(hub: string, account: string, profile: Profile): Promise<number>;
  mcp(hub: string, account: string, call: McpCall): Promise<McpReply>;
}

const KEPT = "profile.account";

export function loginKey(login: Login): string {
  return `${login.hub}/${login.account}`;
}

function kept(): string | null {
  try {
    return localStorage.getItem(KEPT);
  } catch {
    return null;
  }
}

function keep(key: string): void {
  try {
    localStorage.setItem(KEPT, key);
  } catch {
    // Chosen for this visit only.
  }
}

export class AccountChoice {
  /** The place in the head the pill goes. */
  readonly element: HTMLElement;
  private menu: { element: HTMLElement; popover: Popover } | null = null;
  private drawn = "";
  private logins: Login[] = [];
  private key: string | null = kept();

  constructor(
    private readonly words: ProfileWords,
    private readonly changed: (login: Login) => void,
  ) {
    this.element = document.createElement("span");
    this.element.className = "prof-account";
    this.element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const item = target.closest<HTMLElement>("[data-account]");
      if (item) {
        this.menu?.popover.hide();
        const key = item.dataset.account!;
        const current = this.chosen();
        if (current && key === loginKey(current)) return;
        this.key = key;
        keep(key);
        this.draw(null);
        const login = this.chosen();
        if (login) this.changed(login);
      } else if (target.closest("[data-account-open]")) this.open();
    });
  }

  /** The account shown: the one chosen, or the first. */
  chosen(): Login | null {
    return this.logins.find((login) => loginKey(login) === this.key) ?? this.logins[0] ?? null;
  }

  /** Draws the pill as the World now stands; a pill only where there is a
   *  choice. */
  draw(world: WorldNow | null): void {
    if (world) this.logins = loginsOf(world.accounts);
    const chosen = this.chosen();
    const html =
      this.logins.length > 1 && chosen
        ? `<button class="agents-pick" type="button" data-account-open aria-haspopup="menu" aria-label="${escape(this.words.account)}">
            <span class="agents-pick-icon" aria-hidden="true">${glyph("person", 15)}</span><span class="agents-pick-name">${escape(chosen.account)}</span><span class="agents-pick-sub">${escape(chosen.hubName)}</span>${glyph("chevron", 14)}</button>`
        : "";
    if (html === this.drawn) return;
    const open = this.menu?.popover.open ?? false;
    this.menu?.element.remove();
    this.menu = null;
    this.element.innerHTML = html;
    this.drawn = html;
    if (open && html) this.open();
  }

  private open(): void {
    const anchor = this.element.querySelector<HTMLElement>(".agents-pick");
    if (!anchor) return;
    if (this.menu) {
      this.menu.popover.toggle();
      return;
    }
    const chosen = this.chosen();
    const element = document.createElement("div");
    element.className = "map-menu agents-pick-menu";
    element.setAttribute("role", "menu");
    element.innerHTML = `<div class="agents-pick-group" role="group">${this.logins
      .map((login) => {
        const on = chosen !== null && loginKey(login) === loginKey(chosen);
        return `<button class="agents-pick-item${on ? " current" : ""}" type="button" role="menuitemradio" aria-checked="${on}" data-account="${escape(loginKey(login))}">
          <span class="agents-pick-icon" aria-hidden="true">${glyph("person", 15)}</span><span class="agents-pick-name">${escape(login.account)}</span><span class="agents-pick-sub">${escape(login.hubName)}</span></button>`;
      })
      .join("")}</div>`;
    this.element.append(element);
    const popover = new Popover(element, anchor, () => {});
    this.menu = { element, popover };
    popover.show();
    element.querySelector<HTMLElement>(".agents-pick-item.current")?.focus({ preventScroll: true });
  }

  leave(): void {
    this.menu?.element.remove();
    this.menu = null;
  }
}
