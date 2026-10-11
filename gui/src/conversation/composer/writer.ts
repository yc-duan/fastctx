import type { Address } from "../../world/wire";

/** Browser-created windows can inherit sessionStorage. A Web Lock keeps their
 * outboxes separate and prevents two live writers from replaying one recovery key. */
export async function composerWriter(who: Address, requested?: string): Promise<{ key: string; release: () => void }> {
  const storageKey = JSON.stringify(["world-composer-writer", who.machine, who.resident]);
  let key = requested ?? sessionStorage.getItem(storageKey) ?? JSON.stringify([who.machine, who.resident, crypto.randomUUID()]);
  const claim = (key: string): Promise<(() => void) | null> => new Promise((resolve, reject) => {
    void navigator.locks.request(`world-composer:${key}`, { ifAvailable: true }, async (lock) => {
      if (!lock) { resolve(null); return; }
      await new Promise<void>((release) => { resolve(release); });
    }).catch(reject);
  });
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
