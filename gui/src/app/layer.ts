// The account's layer (`world-layers.md`): Connect gives the user's own
// agents `run` and shows what they did; the workbench adds Fastctx's own
// conversations. It is kept with the account, on its hub, and a computer
// in no network keeps its own; this window keeps the last value read so its
// first frame is already right. An account that never chose is asked once,
// unless its device had the workbench on before there were layers, and
// meanwhile has the workbench.

import { invoke } from "@tauri-apps/api/core";
import { signal, type Signal } from "./signals";

export type LayerName = "connect" | "workbench";

const KEPT = "layer";

interface Chosen {
  layer?: LayerName;
}

/** Where the layer is read and kept: the node, or a device's hub. */
export interface LayerSeam {
  /** Whose layer it reads and keeps now: the account, or none. */
  scope(): string;
  read(): Promise<LayerName | null>;
  write(layer: LayerName): Promise<LayerName | null>;
}

/** The layer as the node keeps it, for the account `scope` names: with
 *  several logged in, the one the settings are kept for. */
export function liveLayer(scope: () => { hub: string; account: string } | null): LayerSeam {
  const at = () => {
    const chosen = scope();
    return { hub: chosen?.hub ?? null, account: chosen?.account ?? null };
  };
  return {
    scope: () => {
      const { hub, account } = at();
      return hub && account ? `${hub}/${account}` : "";
    },
    read: () => invoke<Chosen>("layer_read", at()).then((chosen) => chosen.layer ?? null),
    write: (layer) => invoke<Chosen>("layer_set", { ...at(), layer }).then((chosen) => chosen.layer ?? null),
  };
}

class AccountLayer {
  /** None until the account chooses. */
  readonly chosen: Signal<LayerName | null> = signal(kept());
  /** Whether Fastctx's own conversations are shown: the workbench chosen,
   *  or nothing chosen yet. */
  readonly workbench: Signal<boolean> = signal(this.chosen.peek() !== "connect");
  /** Whether the layer was read at least once from where it is kept. */
  readonly known: Signal<boolean> = signal(false);
  /** Whose layer was read last (`LayerSeam.scope`): one read before the
   *  logins were known is this computer's own, not the account's. */
  readonly readFor: Signal<string | null> = signal<string | null>(null);
  private seam: LayerSeam | null = null;

  attach(seam: LayerSeam): void {
    this.seam = seam;
    void this.refresh();
  }

  /** Reads the layer again, as when another device may have changed it. */
  async refresh(): Promise<void> {
    if (!this.seam) return;
    const scope = this.seam.scope();
    try {
      this.set(await this.seam.read());
      this.known.set(true);
      this.readFor.set(scope);
    } catch (error) {
      console.error("reading the account's layer", error);
    }
  }

  async choose(layer: LayerName): Promise<void> {
    const chosen = this.seam ? await this.seam.write(layer) : layer;
    this.set(chosen ?? layer);
    this.known.set(true);
  }

  private set(layer: LayerName | null): void {
    this.chosen.set(layer);
    this.workbench.set(layer !== "connect");
    try {
      if (layer) localStorage.setItem(KEPT, layer);
      else localStorage.removeItem(KEPT);
    } catch {
      // Not kept in this window; read again next time.
    }
  }
}

function kept(): LayerName | null {
  const asked = new URLSearchParams(location.search);
  if (asked.has("conversations") || asked.get("layer") === "workbench") return "workbench";
  if (asked.get("layer") === "connect") return "connect";
  if (asked.has("unchosen")) return null;
  try {
    const value = localStorage.getItem(KEPT);
    return value === "connect" || value === "workbench" ? value : null;
  } catch {
    return null;
  }
}

export const accountLayer = new AccountLayer();
