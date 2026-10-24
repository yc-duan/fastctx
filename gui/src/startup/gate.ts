// Inside the app's shell the window opens before the World on this device
// does, so nothing that needs it is asked for until it has: the interface
// loads once the shell says it is ready. Until then this page stands in, and
// where opening failed it offers a repair of this device's Fastctx and
// another try. In a plain browser, `?startup=opening|failed|repairing|
// repaired|broken` draws the page instead, for looking at it.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inShell, onComputer } from "../world/bridge";
import type { RepairReport, RepairStep } from "../world/repair";
import { StartupPage, type Failure } from "./page";
import { enStartup, zhStartup } from "./words";

interface Status {
  phase: "starting" | "ready" | "failed";
  failure: Failure | null;
}

/** How long opening may take before its line shows. */
const QUIET_MS = 600;
/** How often a repair's record and the opening are read again. */
const POLL_MS = 700;

function language(): "zh" | "en" {
  const asked = new URLSearchParams(location.search).get("lang");
  if (asked === "zh" || asked === "en") return asked;
  try {
    const kept = localStorage.getItem("lang");
    if (kept === "zh" || kept === "en") return kept;
  } catch {
    // Nothing kept in this window.
  }
  return /^zh/i.test(navigator.language) ? "zh" : "en";
}

/** Resolves once the interface may load. */
export async function untilStarted(): Promise<void> {
  const asked = new URLSearchParams(location.search).get("startup");
  if (!inShell()) {
    if (asked) await specimen(asked);
    return;
  }
  const first = await invoke<Status>("startup_state");
  if (first.phase === "ready") return;
  await new Gate().ready;
}

class Gate {
  readonly ready: Promise<void>;
  private resolve!: () => void;
  private readonly page: StartupPage;
  /** The repair this page started, followed until it ends. */
  private repair: string | null = null;
  private shown = false;
  private done = false;

  constructor() {
    this.ready = new Promise((resolve) => (this.resolve = resolve));
    const words = language() === "en" ? enStartup : zhStartup;
    this.page = new StartupPage(words, {
      retry: () => void this.retry(),
      repair: onComputer() ? () => void this.startRepair() : null,
    });
    setTimeout(() => {
      if (!this.shown && !this.done) this.show(() => this.page.opening());
    }, QUIET_MS);
    void listen("startup-changed", () => void this.read());
    void this.tick();
  }

  private show(draw: () => void): void {
    draw();
    if (!this.shown) {
      this.shown = true;
      document.body.prepend(this.page.element);
    }
  }

  /** Reads again on a beat as well as on the shell's word, which may be
   *  missed between the first look and listening. */
  private async tick(): Promise<void> {
    while (!this.done) {
      await this.read();
      await new Promise((resolve) => setTimeout(resolve, POLL_MS));
    }
  }

  private async read(): Promise<void> {
    if (this.done) return;
    if (this.repair) return this.follow();
    const status = await invoke<Status>("startup_state").catch(() => null);
    if (status) this.status(status);
  }

  private status(status: Status): void {
    if (status.phase === "ready") return this.finish();
    if (status.phase === "failed") this.show(() => this.page.failed(status.failure));
    else if (this.shown) this.show(() => this.page.opening());
  }

  private async retry(): Promise<void> {
    this.repair = null;
    this.show(() => this.page.opening());
    const status = await invoke<Status>("startup_retry").catch(() => null);
    if (status) this.status(status);
  }

  private async startRepair(): Promise<void> {
    this.show(() => this.page.repairing([]));
    try {
      const accepted = await invoke<{ id: string }>("repair_here");
      this.repair = accepted.id;
    } catch {
      // One already runs (started from the repair shortcut, say): follow it.
      const running = await invoke<RepairReport | null>("repair_state").catch(() => null);
      this.repair = running && running.finished === null ? running.id : null;
      if (!this.repair) this.show(() => this.page.repairFailed([], null));
    }
  }

  private async follow(): Promise<void> {
    const report = await invoke<RepairReport | null>("repair_state").catch(() => null);
    if (!report || report.id !== this.repair) return;
    if (report.finished === null) return this.show(() => this.page.repairing(report.steps));
    this.repair = null;
    if (!report.success) return this.show(() => this.page.repairFailed(report.steps, report.error));
    this.show(() => this.page.repaired(report.steps));
    const status = await invoke<Status>("startup_retry").catch(() => null);
    if (status) this.status(status);
  }

  private finish(): void {
    this.done = true;
    this.page.element.remove();
    this.resolve();
  }
}

/** The page in each of its states, never resolving, so nothing loads after. */
async function specimen(state: string): Promise<never> {
  const words = language() === "en" ? enStartup : zhStartup;
  const page = new StartupPage(words, { retry: () => {}, repair: () => {} });
  const steps: RepairStep[] = [
    { layer: "package", outcome: "unchanged", detail: "1.0.1-beta.12 verified", actions: [] },
    { layer: "records", outcome: "unchanged", detail: "", actions: [] },
    { layer: "program", outcome: "repaired", detail: "replaced the damaged program", actions: ["installed"] },
    { layer: "service", outcome: state === "broken" ? "failed" : state === "repairing" ? "checking" : "unchanged", detail: state === "broken" ? "the service would not start" : "", actions: [] },
  ];
  if (state !== "repairing" && state !== "broken") steps.push({ layer: "application", outcome: "unchanged", detail: "", actions: [] }, { layer: "node", outcome: "unchanged", detail: "", actions: [] });
  if (state === "opening") page.opening();
  else if (state === "repairing") page.repairing(steps);
  else if (state === "repaired") page.repaired(steps);
  else if (state === "broken") page.repairFailed(steps, "service: systemctl start exited with 1");
  else page.failed({ step: "open_connection", error: "reading connection choice C:\\Users\\me\\AppData\\Roaming\\fastctx\\connection.json: Access is denied. (os error 5)" });
  document.body.prepend(page.element);
  return new Promise<never>(() => {});
}
