// A move of the specimen's server, to be looked at: the machines that can
// take it, and a move that goes a step further each time it is asked about.

import type { HubMove, HubMoveReply, HubMoveRequest, MoveStage } from "../world/hub-move";

const STAGES: MoveStage[] = ["preparing", "service", "reaching", "copying", "switching", "following"];

export function specimenHubMove(): (request: HubMoveRequest) => Promise<HubMoveReply> {
  let move: HubMove | null = null;
  let asked = 0;
  return async (request) => {
    switch (request.request) {
      case "targets":
        return {
          kind: "targets",
          port: 7443,
          machines: [
            { name: "desktop", online: true, os: "windows", form: "desktop", sleeps: false, interfaces: [{ interface: "Ethernet", address: "192.168.1.20", physical: true }], hosts_hub: false },
            { name: "laptop", online: true, os: "windows", form: "laptop", sleeps: true, interfaces: [{ interface: "Wi-Fi", address: "192.168.1.31", physical: true }], hosts_hub: false },
            { name: "lab", online: true, os: "linux", form: "server", sleeps: false, seen: "203.0.113.30:51022", interfaces: [{ interface: "eth0", address: "10.0.0.4", physical: true }], hosts_hub: false },
            { name: "server", online: true, os: "linux", form: "server", sleeps: false, seen: "203.0.113.40:40112", hosts_hub: true, unavailable: "the hub runs there now" },
            { name: "studio", online: false, os: "windows", form: "desktop", sleeps: false, hosts_hub: false, unavailable: "it is offline" },
          ],
        };
      case "start":
        move = { id: "mv-7f3a", to: request.to, addresses: request.addresses, from: ["203.0.113.10:7443"], by: "me", started: new Date().toISOString(), stage: "preparing" };
        asked = 0;
        return { kind: "moves", moves: [move] };
      case "status": {
        if (move && move.stage !== "cancelled") {
          asked += 1;
          // A step every few looks; the service waits for its button.
          const index = Math.min(STAGES.length - 1, Math.floor(asked / 3));
          const stage = STAGES[index]!;
          const { waiting: _, ...rest } = move;
          move = {
            ...rest,
            stage,
            ...(stage === "service" ? { waiting: { kind: "service" as const, dir: "/root/.fastctx/hub", os: "linux" } } : {}),
            ...(stage === "following"
              ? { switched: new Date().toISOString(), following: { machines: ["studio"], devices: ["phone"], old: { state: "forwarding" as const }, old_connected: true } }
              : {}),
          };
        }
        return { kind: "moves", moves: move ? [move] : [] };
      }
      case "cancel":
        if (move) move = { ...move, stage: "cancelled" };
        return { kind: "moves", moves: move ? [move] : [] };
      case "retry":
        return { kind: "moves", moves: move ? [move] : [] };
      case "stop_forwarding":
        if (move?.following) move = { ...move, following: { ...move.following, old: { state: "stopped", at: new Date().toISOString(), early: true } } };
        return { kind: "moves", moves: move ? [move] : [] };
      case "delete_old":
        if (move?.following) move = { ...move, stage: "done", following: { ...move.following, old: { state: "deleted", at: new Date().toISOString() } } };
        return { kind: "moves", moves: move ? [move] : [] };
    }
  };
}
