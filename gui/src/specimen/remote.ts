// A server a specimen makes a World on: it shows a fingerprint, ticks each
// step after a moment, and signs in as the World would have.

import type { RemoteSeams, RemoteStep } from "../app/remote-world";
import { specimenNameProblem } from "./names";

export function specimenRemote(): RemoteSeams {
  const after = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
  const steps = async (names: RemoteStep[], step: (step: RemoteStep) => void) => {
    for (const name of names) {
      step(name);
      await after(700);
    }
  };
  return {
    probe: async (host, port) => ({ address: `203.0.113.20:${port}`, fingerprint: "SHA256:q1mWkC3Pz0b0n5yVZpA8k7m2sQ0Jt9eYxH4fR6uLwNc", known: host === "known" ? "known" : "unknown" }),
    look: async () => {
      await after(900);
      return "izbp19mwh1hladmc1jby2gz";
    },
    hub: async (request, step) => {
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
    login: async (_address, admin) => {
      await after(500);
      return { hub: "home", name: "desktop", account: admin, kept: { kind: "installed" as const } };
    },
    token: async () => "FX5SPECIMEN",
    nameProblem: specimenNameProblem,
  };
}
