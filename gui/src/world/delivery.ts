// Observation payloads stay in the shell's bounded queue. A Channel only wakes
// this reader; each pull acknowledges the preceding chunk after its callback ran.
import { Channel, invoke } from "@tauri-apps/api/core";

type Interrupted = { kind: "interrupted"; error: string; since: number };
type Read =
  | { kind: "chunk"; number: number; offset: number; total: number; text: string }
  | { kind: "gap"; number: number }
  | { kind: "idle" }
  | { kind: "closed" };

type Opened = number | { id: number };
interface Reader { wake(): void; close(): void }
const readers = new Map<number, Reader>();
const encoder = new TextEncoder();
const gap = "the interface fell behind; reading the current state again";

export function closeDelivery(id: number): void {
  readers.get(id)?.close();
  readers.delete(id);
}

/** Foreground/network recovery also retries a local invocation that lost its reply. */
export function retryDeliveries(): void {
  for (const reader of readers.values()) reader.wake();
}

/** Public watch ids stay unchanged when the shell replaces a lagged subscription. */
export async function observe<T, R extends Opened>(
  command: string,
  args: Record<string, unknown>,
  onPiece: (piece: T | Interrupted) => void,
): Promise<R> {
  let id: number | null = null;
  let through = 0;
  let running = false;
  let requested = false;
  let closed = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let failures = 0;
  let parts: string[] = [];
  let received = 0;
  let total = 0;
  // A part that cannot be read is acknowledged and answered with a gap: the
  // source starts again from what is now, not the same part again.
  let resync = false;
  const unreadable = (number: number, why: string) => {
    console.error("an observation part could not be read:", why);
    parts = [];
    received = total = 0;
    through = number;
    resync = true;
  };
  const deliver = (piece: T | Interrupted) => {
    try { onPiece(piece); }
    catch (error) { console.error("an observation could not be shown", error); }
  };
  const reader: Reader = {
    close: () => {
      closed = true;
      parts = [];
      if (timer !== null) clearTimeout(timer);
      timer = null;
    },
    wake: () => {
      if (closed) return;
      requested = true;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      if (!running && id !== null) void pump();
    },
  };
  const bind = (number: number) => {
    if (id !== null && id !== number) throw new Error("the observation changed its watch id");
    id = number;
    if (!closed) readers.set(number, reader);
  };
  async function pump(): Promise<void> {
    running = true;
    try {
      do {
        requested = false;
        while (!closed) {
          const read = await invoke<Read>("watch_delivery_read", { id, through, resync });
          if (closed) return;
          failures = 0;
          resync = false;
          if (read.kind === "idle") break;
          if (read.kind === "closed") {
            reader.close();
            if (id !== null) readers.delete(id);
            return;
          }
          if (read.number !== through + 1) {
            unreadable(read.number, "out of order");
            continue;
          }
          if (read.kind === "gap") {
            parts = [];
            received = total = 0;
            deliver({ kind: "interrupted", error: gap, since: Date.now() });
          } else {
            const bytes = encoder.encode(read.text).byteLength;
            if (read.offset !== received || read.total < 1 || read.total > 64 * 1024 * 1024 || bytes < 1 || bytes > 64 * 1024
              || received + bytes > read.total || (received !== 0 && total !== read.total)) {
              unreadable(read.number, "out of bounds");
              continue;
            }
            total = read.total;
            received += bytes;
            parts.push(read.text);
            if (received === total) {
              let piece: T;
              try {
                piece = JSON.parse(parts.join(""));
              } catch {
                unreadable(read.number, "not JSON");
                continue;
              }
              parts = [];
              received = total = 0;
              deliver(piece);
            }
          }
          through = read.number;
        }
      } while (requested && !closed);
    } catch (error) {
      if (!closed) {
        deliver({ kind: "interrupted", error: String(error), since: Date.now() });
        const delay = Math.min(10_000, 250 * 2 ** Math.min(failures++, 6));
        timer = setTimeout(() => { timer = null; reader.wake(); }, delay);
      }
    } finally {
      running = false;
    }
  }
  const pieces = new Channel<{ id: number }>();
  pieces.onmessage = (notice) => {
    try { bind(notice.id); reader.wake(); }
    catch (error) { console.error("an observation wakeup could not be read", error); }
  };
  try {
    const opened = await invoke<R>(command, { ...args, pieces });
    bind(typeof opened === "number" ? opened : opened.id);
    return opened;
  } catch (error) {
    reader.close();
    if (id !== null) readers.delete(id);
    throw error;
  }
}
