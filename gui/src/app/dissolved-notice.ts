// Said once on each computer or phone that was on networks their
// administrators dissolved (`world-network.md`, 解散网络): a sheet names them
// and that Fastctx and the data here stay. However it is closed, they are
// not said again.

import { escape } from "../conversation/prose";
import type { Dissolution } from "../world/accounts";
import { Sheet } from "./confirm";
import { effect } from "./signals";
import type { Store } from "./store";

export interface DissolvedWords {
  dissolvedTitle(hubs: string[]): string;
  dissolvedWhat(phone: boolean, many: boolean): string;
  dissolvedSeen: string;
}

export interface DissolvedOptions {
  store: Store;
  words: DissolvedWords;
  close: string;
  phone: boolean;
  /** Tells the World they were seen. */
  seen(): Promise<void>;
}

export function noticeDissolved(options: DissolvedOptions): void {
  const { store, words } = options;
  const sheet = new Sheet("device-sheet confirm-sheet dissolved-sheet");
  // What was said this time the window is open, so a World that has not
  // heard yet that they were seen does not say them again.
  const said = new Set<string>();
  const key = (one: Dissolution) => `${one.hub}@${one.at}`;
  sheet.body.onsubmit = (event) => {
    event.preventDefault();
    sheet.close();
  };
  sheet.dialog.addEventListener("close", () => {
    options.seen().catch((error: unknown) => console.warn("the dissolved networks could not be marked seen", error));
  });
  effect(() => {
    const dissolved = (store.world.get()?.accounts.dissolved ?? []).filter((one) => !said.has(key(one)));
    if (!dissolved.length || sheet.dialog.open) return;
    for (const one of dissolved) said.add(key(one));
    const hubs = [...new Set(dissolved.map((one) => one.hub))];
    sheet.body.innerHTML = `
      ${sheet.head(words.dissolvedTitle(hubs), options.close)}
      <p class="confirm-what">${escape(words.dissolvedWhat(options.phone, hubs.length > 1))}</p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.dissolvedSeen)}</button></footer>`;
    sheet.show();
  });
}
