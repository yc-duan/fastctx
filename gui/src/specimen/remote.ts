// A server a specimen makes a World on: it shows a fingerprint, ticks each
// step after a moment, and signs in as the World would have.

import type { RemoteSeams, RemoteStep } from "../app/remote-world";
import { specimenNameProblem } from "./names";

export function specimenRemote(): RemoteSeams {
  // A host named shut makes its hub and is not reached at it, as behind a
  // port the security group does not let through.
  let shut = false;
  const after = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
  const steps = async (names: RemoteStep[], step: (step: RemoteStep) => void) => {
    for (const name of names) {
      step(name);
      await after(700);
    }
  };
  return {
    probe: async (host, port) => ({ address: `203.0.113.20:${port}`, fingerprint: "SHA256:q1mWkC3Pz0b0n5yVZpA8k7m2sQ0Jt9eYxH4fR6uLwNc", known: host === "known" ? "known" : "unknown" }),
    // A host named found keeps a World already, half made at half; one
    // named busy has the usual ports taken.
    look: async (server) => {
      await after(900);
      const hub = server.host.includes("half")
        ? { name: null, port: null, address: null }
        : server.host.includes("found")
          ? { name: "home", port: 7443, address: "203.0.113.20:7443" }
          : null;
      const busy = server.host.includes("busy") ? [22, 80, 443, 7443] : [22];
      return { machine: "izbp19mwh1hladmc1jby2gz", busy, hub, joined: false };
    },
    withdraw: async (_server, step) => steps(["connect", "check", "upload", "withdraw"], step),
    logout: async () => {},
    hub: async (request, step) => {
      shut = request.host.includes("shut");
      await steps(["connect", "check", "upload", "hub"], step);
      return { session: 1, address: `203.0.113.20:${request.hubPort}`, machine: "hz-light-01" };
    },
    open: async (_server, step) => {
      await steps(["connect", "check", "upload"], step);
      return { session: 2, machine: "gz-cvm-07" };
    },
    join: async (_session, _token, step) => steps(["join"], step),
    arrive: async (_session, step) => steps(["arrive"], step),
    close: async () => {},
    login: async (address, admin) => {
      await after(500);
      if (shut) throw new Error(`reaching ${address}: connection timed out`);
      return { hub: "home", name: "desktop", account: admin, kept: { kind: "installed" as const } };
    },
    token: async () => "FX5SPECIMEN",
    nameProblem: specimenNameProblem,
  };
}
