// New versions in the specimen, for a plain browser: a test release newer
// than this one, whose package comes in a few seconds, through the network
// when the specimen says GitHub cannot be reached. `?update=none` offers
// nothing; `?update=ready` has the package here already; `?update=offline`
// cannot reach GitHub and hears of the release from the network.

import type { Manifest, UpdateSeam, UpdateSettings, UpdateState } from "../world/updates";

const SIZE = 14_200_000;
const TAKES = 6_000;

export function specimenUpdates(params: URLSearchParams, phone: boolean): { seam: UpdateSeam; version: string } {
  const version = "1.0.1-beta.1";
  const target = phone ? "android-apk" : "windows-x86_64-nsis";
  const offline = params.get("update") === "offline";
  const offer: Manifest = {
    version: "1.0.1-beta.2",
    notes: "Updates come through the network, and the conversation's stop button stops at once.",
    platforms: { [target]: { signature: "", size: SIZE } },
  };
  let settings: UpdateSettings = { auto: false, relays: !phone };
  let started: number | null = params.get("update") === "ready" ? 0 : null;
  const state = (): UpdateState => {
    const coming = started !== null && Date.now() - started < TAKES;
    const here = started !== null && !coming;
    const none = params.get("update") === "none";
    return {
      settings,
      channel: settings.beta === false ? "stable" : "beta",
      looked: { at: new Date().toISOString(), github: !offline, ...(offline ? { why: "GitHub did not answer" } : { beta: offer }) },
      network: { beta: offer, policy: { channel: "beta" }, machines: {} },
      ...(none || settings.beta === false ? {} : { offer }),
      ...(coming
        ? { fetching: { version: offer.version, target, via: offline ? "network" : "github", received: Math.floor(((Date.now() - started!) / TAKES) * SIZE), total: SIZE } }
        : {}),
      kept: here ? [{ version: offer.version, target, path: "" }] : [],
      app: { version, target },
      kernel_target: "",
    };
  };
  const seam: UpdateSeam = {
    state: async () => state(),
    look: async () => {
      await new Promise((done) => setTimeout(done, 600));
      return state();
    },
    set: async (next) => {
      settings = next;
      return state();
    },
    fetch: async () => {
      started ??= Date.now();
      return state();
    },
    install: async () => {
      await new Promise((done) => setTimeout(done, 1_500));
      return true;
    },
    machine: async () => state(),
  };
  return { seam, version };
}
