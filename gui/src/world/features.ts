// The optional parts of the interface this device shows (`world-gui.md`,
// 功能), remembered by the device: on a computer its node keeps them, so the
// TUI shows the same; a phone keeps its own.

import { invoke } from "@tauri-apps/api/core";
import { observe } from "./delivery";

export interface Features {
  /** Talking to agents here: home, new conversations, projects and
   *  conversations in the sidebar, the notifications that ask. */
  conversations: boolean;
  /** The language chosen, by its tag, `zh` or `en`; none until chosen. */
  language?: string;
}

export type Feature = "conversations";

export type FeaturesPiece =
  | { kind: "features"; features: Features }
  | { kind: "interrupted"; error: string; since: number };

export function readFeatures(): Promise<Features> {
  return invoke("features_read");
}

/** Turns one switch on or off; all of them after. */
export function setFeature(feature: Feature, on: boolean): Promise<Features> {
  return invoke("features_set", { feature, on });
}

/** Says the interface speaks the language tagged `language`; all the
 *  switches after. */
export function setLanguage(language: string): Promise<Features> {
  return invoke("features_language", { language });
}

/** Follows the switches: first as they are, then after each change, from
 *  whichever interface changed them. Resolves to the id `unwatch` takes. */
export function watchFeatures(onPiece: (piece: FeaturesPiece) => void): Promise<number> {
  return observe<FeaturesPiece, number>("features_watch", {}, onPiece);
}
