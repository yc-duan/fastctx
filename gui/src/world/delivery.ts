// Observation pieces come pushed over a Channel, in numbered batches, as the
// shell's bounded queue lets them go: at most a window the page has not yet
// acknowledged. Each batch is acknowledged once its pieces ran, a few at a
// time, without waiting for the answer; a batch missed is asked for again.
import { Channel, invoke } from "@tauri-apps/api/core";

type Interrupted = { kind: "interrupted"; error: string; since: number; gap?: boolean };
type Push =
  | { kind: "pieces"; id: number; number: number; pieces: unknown[] }
  | { kind: "gap"; id: number; number: number }
  | { kind: "closed"; id: number };

type Opened = number | { id: number };
interface Reader { wake(): void; close(): void }
const readers = new Map<number, Reader>();
const gap = "the interface fell behind; reading the current state again";
/** How long acknowledgements gather before one goes: about a frame. */
const ACK_MS = 16;

export function closeDelivery(id: number): void {
  readers.get(id)?.close();
  readers.delete(id);
}

/** Foreground/network recovery asks again for what a stream may have missed. */
export function retryDeliveries(): void {
  for (const reader of readers.values()) reader.wake();
}

/** Public watch ids stay unchanged when the shell replaces a lagged subscription. */
export async function observe<T, R extends Opened | null>(
  command: string,
  args: Record<string, unknown>,
  onPiece: (piece: T | Interrupted) => void,
): Promise<R> {
  let id: number | null = null;
  let through = 0;
  let closed = false;
  let acking: ReturnType<typeof setTimeout> | null = null;
  let asked = false;
  const deliver = (piece: T | Interrupted) => {
    try { onPiece(piece); }
    catch (error) { console.error("an observation could not be shown", error); }
  };
  const acknowledge = (now = false) => {
    if (id === null) return;
    const send = () => {
      acking = null;
      if (id !== null) invoke("watch_delivery_ack", { id, through }).catch((error) => console.error("an observation acknowledgement failed", error));
    };
    if (now) {
      if (acking !== null) clearTimeout(acking);
      send();
    } else if (acking === null) {
      acking = setTimeout(send, ACK_MS);
    }
  };
  const reader: Reader = {
    close: () => {
      closed = true;
      if (acking !== null) clearTimeout(acking);
      acking = null;
    },
    wake: () => {
      if (closed || id === null) return;
      invoke("watch_delivery_resend", { id, through }).catch((error) => console.error("asking an observation again failed", error));
    },
  };
  const bind = (number: number) => {
    if (id !== null && id !== number) throw new Error("the observation changed its watch id");
    id = number;
    if (!closed) readers.set(number, reader);
  };
  const take = (push: Push) => {
    if (closed) return;
    bind(push.id);
    if (push.kind === "closed") {
      acknowledge(true);
      reader.close();
      readers.delete(push.id);
      return;
    }
    // A batch pushed again that ran already.
    if (push.number <= through) return;
    if (push.number !== through + 1) {
      // One went missing on the way: what came after it is asked for again.
      if (!asked) {
        asked = true;
        reader.wake();
      }
      return;
    }
    asked = false;
    through = push.number;
    if (push.kind === "gap") {
      deliver({ kind: "interrupted", error: gap, since: Date.now(), gap: true });
      acknowledge(true);
      return;
    }
    for (const piece of push.pieces) deliver(piece as T);
    acknowledge();
  };
  const pieces = new Channel<Push>();
  pieces.onmessage = (push) => {
    try { take(push); }
    catch (error) { console.error("an observation batch could not be read", error); }
  };
  try {
    const opened = await invoke<R>(command, { ...args, pieces });
    if (opened === null) { reader.close(); return opened; }
    bind(typeof opened === "number" ? opened : opened.id);
    return opened;
  } catch (error) {
    reader.close();
    if (id !== null) readers.delete(id);
    throw error;
  }
}
