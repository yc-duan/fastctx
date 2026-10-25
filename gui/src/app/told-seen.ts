// What of the agents' words to the user this device has shown them
// (`world-voice.md`, 要紧的事): for each conversation, the last thing it told
// that its page was in view for. What it tells after that waits on the
// project's page and the home page until it is opened. Kept on this device,
// as the choice of view is.

const KEY = "toldSeen";
/** The most conversations remembered; the least lately seen go first. */
const KEPT = 400;

function read(): Record<string, string> {
  try {
    const kept = JSON.parse(localStorage.getItem(KEY) ?? "{}") as unknown;
    return kept && typeof kept === "object" ? (kept as Record<string, string>) : {};
  } catch {
    return {};
  }
}

/** The last told message of `key` (`resident@machine`) seen here, if any. */
export function toldSeen(key: string): string | null {
  return read()[key] ?? null;
}

/** `id`, told by `key`, has been in view here. */
export function sawTold(key: string, id: string): void {
  const kept = read();
  if (kept[key] === id) return;
  delete kept[key];
  kept[key] = id;
  const keys = Object.keys(kept);
  for (const old of keys.slice(0, Math.max(0, keys.length - KEPT))) delete kept[old];
  try {
    localStorage.setItem(KEY, JSON.stringify(kept));
  } catch {
    // Not kept: what was told waits on the project's page until seen again.
  }
}
