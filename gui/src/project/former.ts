// Historical town residents are read on entry and after a move. A failed read
// remains wanted; visibility restoration also checks for missed history.
import { Backoff } from "../world/backoff";
import type { WorldWho } from "../world/world";

export function followFormer(read: () => Promise<WorldWho>, receive: (who: WorldWho) => void): { changed(): void; close(): void } {
  const backoff = new Backoff();
  let alive = true;
  let wanted = true;
  let reading = false;
  let due = 0;
  let timer = 0;
  const ask = () => {
    clearTimeout(timer);
    if (!alive || document.hidden || !wanted || reading) return;
    if (Date.now() < due) {
      timer = window.setTimeout(ask, due - Date.now());
      return;
    }
    wanted = false;
    reading = true;
    void Promise.resolve().then(read).then((who) => {
      if (!alive) return;
      backoff.reset();
      due = 0;
      receive(who);
    }).catch(() => {
      wanted = true;
      due = Date.now() + backoff.failed();
    }).finally(() => {
      reading = false;
      ask();
    });
  };
  const visible = () => {
    if (document.hidden) { clearTimeout(timer); return; }
    wanted = true;
    due = 0;
    ask();
  };
  document.addEventListener("visibilitychange", visible);
  ask();
  return {
    changed() { wanted = true; ask(); },
    close() { alive = false; clearTimeout(timer); document.removeEventListener("visibilitychange", visible); },
  };
}
