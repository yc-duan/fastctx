// How this computer's Fastctx is doing, as its node says: the JSON form of
// `Reply::DoctorReadings` (`fastctx doctor readings`). Asked of the node
// here alone; a phone has none to ask.

import { invoke } from "@tauri-apps/api/core";
import type { Timestamp } from "./wire";

/** Its health over the last minute (`fastctx_types::health::Health`). */
export interface NodeHealth {
  /** Work waited over a second for a thread. */
  busy?: boolean;
  /** The longest it waited, in milliseconds. */
  held_up_ms?: number;
  /** Things that have not moved for half a minute. */
  stuck?: number;
  /** Rules of its threads broken since it started. */
  violations?: number;
  /** A bound near its limit, with how much was used in hundredths. */
  near?: { bound: string; used: number } | null;
}

/** A rule of its threads broken (`fastctx_types::readings::Violation`). */
export interface Violation {
  kind: string;
  place: string;
  count: number;
  doing?: string;
  worst_ms?: number;
  last: Timestamp;
}

export interface DoctorReadings {
  readings: { violations?: Violation[]; violations_total?: number };
  health: NodeHealth;
  /** The scenes it left, oldest first. */
  scenes: string[];
}

export function doctorReadings(): Promise<DoctorReadings> {
  return invoke("doctor_readings");
}
