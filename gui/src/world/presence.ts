/** A node or device's Look decision; absence of this field in an older World means unknown. */
export interface MachinePresence {
  machine: string;
  hubs: {
    hub: string;
    revision: number;
    access: "granted" | "not_granted" | "unknown" | "stopped";
  }[];
}
