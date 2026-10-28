import type { Address } from "../../world/wire";
import { composerRecoveries } from "./journal";

/** Whether the recovery key `key` is a writer's of the conversation `who`. */
function writes(key: string, who: Address): boolean {
  try {
    const [machine, resident] = JSON.parse(key) as unknown[];
    return machine === who.machine && resident === who.resident;
  } catch {
    return false;
  }
}

/** Browser-created windows can inherit sessionStorage. A Web Lock keeps their
 * outboxes separate and prevents two live writers from replaying one recovery key.
 * A page that keeps no key, as one opened anew or after the app started
 * again, first takes up an outbox of this conversation that no live page
 * holds: what a closed one had not sent yet is sent, not left unread. */
export async function composerWriter(who: Address, requested?: string): Promise<{ key: string; release: () => void }> {
  const storageKey = JSON.stringify(["world-composer-writer", who.machine, who.resident]);
  const claim = (key: string): Promise<(() => void) | null> => new Promise((resolve, reject) => {
    void navigator.locks.request(`world-composer:${key}`, { ifAvailable: true }, async (lock) => {
      if (!lock) { resolve(null); return; }
      await new Promise<void>((release) => { resolve(release); });
    }).catch(reject);
  });
  const kept = requested ?? sessionStorage.getItem(storageKey);
  if (kept === null) {
    for (const left of await composerRecoveries().catch(() => [] as string[])) {
      if (!writes(left, who)) continue;
      const release = await claim(left);
      if (release) {
        sessionStorage.setItem(storageKey, left);
        return { key: left, release };
      }
    }
  }
  let key = kept ?? JSON.stringify([who.machine, who.resident, crypto.randomUUID()]);
  let release = await claim(key);
  if (!release) {
    if (requested) throw new Error("the composer recovery key is in use by another page");
    key = JSON.stringify([who.machine, who.resident, crypto.randomUUID()]);
    release = await claim(key);
    if (!release) throw new Error("the new composer writer identity is already in use");
  }
  sessionStorage.setItem(storageKey, key);
  return { key, release };
}
