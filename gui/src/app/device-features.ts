// The features that change how this device's interface looks
// (`world-gui.md`, 功能): kept per device, not per account. The workbench is the
// first: off, the interface is this computer's agents, its machines and the
// features; on, the front page, new conversations, the projects and the
// conversations grow in the sidebar. A computer's interface and its TUI share
// the value through the node; a phone keeps its own.

import { signal, type Signal } from "./signals";

const KEPT = "feature.conversations";

/** Where the value lives when not in this window: the node, on a computer;
 *  the device's own directory, on a phone. `follow` hears a change made
 *  elsewhere, such as in the TUI. */
export interface FeatureSeam {
  read(): Promise<boolean>;
  write(on: boolean): Promise<void>;
  follow(heard: (on: boolean) => void): void;
}

class DeviceFeatures {
  readonly conversations: Signal<boolean> = signal(kept());
  private seam: FeatureSeam | null = null;

  /** Reads the value from where the device keeps it, and keeps it there
   *  from now on. */
  attach(seam: FeatureSeam): void {
    this.seam = seam;
    seam.follow((on) => {
      this.conversations.set(on);
      keep(on);
    });
    void seam.read().then(
      (on) => {
        this.conversations.set(on);
        keep(on);
      },
      (error: unknown) => console.error("reading this device's features", error),
    );
  }

  async setConversations(on: boolean): Promise<void> {
    if (this.seam) await this.seam.write(on);
    keep(on);
    this.conversations.set(on);
  }
}

function kept(): boolean {
  if (new URLSearchParams(location.search).has("conversations")) return true;
  try {
    return localStorage.getItem(KEPT) === "1";
  } catch {
    return false;
  }
}

function keep(on: boolean): void {
  try {
    localStorage.setItem(KEPT, on ? "1" : "0");
  } catch {
    // Not kept in this window: the device's own record still holds it.
  }
}

export const deviceFeatures = new DeviceFeatures();
