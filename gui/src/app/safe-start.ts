// The interface's safe start (`gui/src-tauri/src/ui_watch.rs`): the shell
// opened the window again because the page stopped answering, and the page
// opens on the agents without bringing back where it was. What was typed and
// not sent stays where it was kept. The page says so once, and forgets that
// it started so, so the next load opens as ever.

import { escape } from "../conversation/prose";
import { Sheet } from "./confirm";

export interface SafeStartWords {
  title: string;
  what: string;
  seen: string;
  close: string;
}

export const zhSafeStart: Omit<SafeStartWords, "close"> = {
  title: "界面已重新打开",
  what: "刚才界面没有响应，Fastctx 把窗口重新打开了，这次没有回到上次的页面。没发出去的文字都还在。",
  seen: "知道了",
};

export const enSafeStart: Omit<SafeStartWords, "close"> = {
  title: "The window was opened again",
  what: "The interface stopped answering, so Fastctx opened its window again, this time without going back to where it was. Nothing you typed and had not sent is lost.",
  seen: "OK",
};

/** Whether this load is the shell's safe start. */
export const safeStart = new URLSearchParams(location.search).has("safe");

/** Says once that the window was opened again, and takes the safe start
 *  out of the address, so a load after this one opens as ever. */
export function noticeSafeStart(words: SafeStartWords): void {
  if (!safeStart) return;
  history.replaceState(null, "", `${location.pathname}${location.hash}`);
  const sheet = new Sheet("device-sheet confirm-sheet");
  sheet.body.onsubmit = (event) => {
    event.preventDefault();
    sheet.close();
  };
  sheet.body.innerHTML = `
    ${sheet.head(words.title, words.close)}
    <p class="confirm-what">${escape(words.what)}</p>
    <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.seen)}</button></footer>`;
  sheet.show();
}
