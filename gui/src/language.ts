// Which language the interface speaks (`world-gui.md`, 第一次打开): the one
// named in the address, or the one this window kept, or the one the device
// keeps, a computer's node sharing it with its TUI. While none is chosen,
// the first screen asks, the system's language offered first. Settled
// before the interface loads, which reads it from this window as it starts.

import { readFeatures, setLanguage } from "./world/features";
import { inShell } from "./world/bridge";

export type Language = "zh" | "en";

const KEPT = "lang";
/** How long the device is waited for before the interface starts as this
 *  window last spoke, or asks. */
const WAIT = 1500;

function kept(): Language | null {
  try {
    const value = localStorage.getItem(KEPT);
    return value === "zh" || value === "en" ? value : null;
  } catch {
    return null;
  }
}

export function keepLanguage(language: Language): void {
  try {
    localStorage.setItem(KEPT, language);
  } catch {
    // Not kept in this window: the device's own record still holds it.
  }
}

/** The language the system's display speaks, offered first. */
export function systemLanguage(): Language {
  return /^zh/i.test(navigator.language) ? "zh" : "en";
}

/** The device's language, unless it does not answer in time. */
async function devices(): Promise<Language | null> {
  const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), WAIT));
  const read = readFeatures().then(
    (features) => (features.language === "zh" || features.language === "en" ? features.language : null),
    () => null,
  );
  return Promise.race([read, timeout]);
}

/** Settles the language before the interface starts: asks for it once,
 *  where nothing chose one yet. */
export async function settleLanguage(): Promise<void> {
  const params = new URLSearchParams(location.search);
  const asked = params.get("lang");
  if (asked === "zh" || asked === "en") return;
  const shell = inShell();
  const here = kept();
  if (here) {
    // Chosen in this window before the device kept one: the device learns
    // it, so the TUI speaks it too.
    if (shell) void devices().then((device) => (device ? undefined : setLanguage(here).catch(() => undefined)));
    return;
  }
  if (shell) {
    const device = await devices();
    if (device) return keepLanguage(device);
  } else if (!params.has("first-language")) return;
  const { askLanguage } = await import("./app/first-language");
  const chosen = await askLanguage(systemLanguage());
  keepLanguage(chosen);
  if (shell) await setLanguage(chosen).catch((error: unknown) => console.error("keeping the language chosen", error));
}
