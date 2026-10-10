/** Windows shell mechanics. The caller decides what merits a notification;
 *  successful notify means Windows accepted it, not that a banner was seen. */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inShell } from "./bridge";

export interface WindowPresence {
  /** Shown and not minimized; another window may still cover it. */
  visible: boolean;
  focused: boolean;
  minimized: boolean;
}

export interface DesktopNotice {
  title: string;
  body: string;
  route: string;
  /** A quiet button hands its choice to the page without bringing the
   *  window forward: an answer given from the notification. */
  actions?: { id: string; title: string; quiet?: boolean }[];
}

export interface DesktopActivation {
  route: string | null;
  action: string | null;
  notificationId: string | null;
}

export type DesktopEvent =
  | ({ kind: "activation" } & DesktopActivation)
  | { kind: "notificationFailed"; notificationId: string; error: string }
  | { kind: "error"; error: string };

export const desktopAvailable = () => inShell() && navigator.userAgent.includes("Windows");
const command = <T>(name: string, args?: Record<string, unknown>) => invoke<T>(`plugin:desktop-presence|${name}`, args);

/** Rejects with the actual native failure. Later Windows failures arrive
 *  through connectDesktop's onError callback with this notification ID. */
export const notifyDesktop = (notice: DesktopNotice): Promise<string> => command("notify", { notice });
export const dismissDesktopNotice = (notificationId: string): Promise<void> => command("dismiss", { notificationId });
export const getWindowPresence = (): Promise<WindowPresence> => command("window_presence");

/** Install once for the page lifetime. Handlers must finish before an event
 *  is acknowledged; reloads can therefore resume an unfinished delivery.
 *  Action failures are reported and acknowledged, never silently retried.
 *  No notification suppression policy lives in this bridge. */
export async function connectDesktop(options: {
  onActivation: (activation: DesktopActivation) => void | Promise<void>;
  onError: (error: string, notificationId: string | null) => void;
  onPresence?: (presence: WindowPresence) => void;
}): Promise<() => void> {
  let running: Promise<void> | null = null;
  let again = false;
  let disposed = false;
  const drain = (): Promise<void> => {
    again = true;
    if (running) return running;
    running = (async () => {
      while (again && !disposed) {
        again = false;
        const deliveries = await command<{ id: number; event: DesktopEvent }[]>("pending");
        for (const { id, event } of deliveries) {
          if (disposed) return;
          if (event.kind === "activation") {
            try { await options.onActivation(event); }
            catch (error) { options.onError(String(error), event.notificationId); }
          } else options.onError(event.error, event.kind === "notificationFailed" ? event.notificationId : null);
          await command("acknowledge", { id });
        }
      }
    })().finally(() => { running = null; });
    return running;
  };
  const onWake = () => { void drain().catch((error: unknown) => options.onError(String(error), null)); };
  const updatePresence = () => {
    void getWindowPresence().then((presence) => {
      if (!disposed) options.onPresence?.(presence);
    }).catch((error: unknown) => options.onError(String(error), null));
  };
  const unlistenPending = await listen("desktop-presence-pending", onWake);
  try {
    const unlistenPresence = await listen("desktop-presence-changed", updatePresence);
    try {
      await drain();
      updatePresence();
      return () => { disposed = true; unlistenPending(); unlistenPresence(); };
    } catch (error) { unlistenPresence(); throw error; }
  } catch (error) { unlistenPending(); throw error; }
}
