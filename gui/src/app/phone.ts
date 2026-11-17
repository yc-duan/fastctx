// The interface on an Android phone, beside the page: the phone calls the
// user when an agent starts waiting for them or a conversation they opened
// finishes, and when something goes wrong with a machine or the network,
// even with the app in the background; a tap on that call opens
// the conversation; and the settings say what the phone lets the World do
// in the background and where to change it. Watching who is waiting runs in
// the app's own process, without the page (`src-tauri/src/android`); the
// page gives it the rules and their words, the same as on a computer
// (`presence.ts`), and routes the taps.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inShell } from "../world/bridge";
import { escape } from "../conversation/prose";
import type { Trouble } from "../world/accounts";
import type { Place } from "./places";
import type { PresenceWords } from "./presence";
import type { TroubleWords } from "./trouble-words";
import type { SettingsWords } from "./words";
import { Sheet } from "./confirm";

export const onPhone = (): boolean => inShell() && /Android/i.test(navigator.userAgent);

const call = <T>(command: string, args?: Record<string, unknown>) => invoke<T>(`plugin:android-world|${command}`, args);

interface Rule {
  when: "asked" | "told" | "finished";
  userStartedOnly: boolean;
  /** One title, or one for each thing an agent may ask, or for each way
   *  it tells the user something worth a call. */
  title: string | Record<string, string>;
  /** The body of a turn that compacted the conversation and said nothing else. */
  compacted?: string;
}

const ASKED_FOR = ["answer", "command", "change", "tool", "plan", "form", "other", "waiting"] as const;

/** Gives the phone the rules it calls the user by, in the interface's
 *  words, or turns them off. The same rules as a computer's: an agent that
 *  asks the user something, a colleague that tells them a reply or
 *  something urgent (what can wait is not pushed), and the answer that ends
 *  a turn the user's words began in a conversation opened through the
 *  World. The phone writes the body from the agent's own words, as a
 *  computer does. */
export function configurePhone(words: PresenceWords, enabled: boolean): Promise<void> {
  const who = "{who}";
  const rules: Rule[] = [
    { when: "asked", userStartedOnly: false, title: Object.fromEntries(ASKED_FOR.map((what) => [what, words.asked(who, what)])) },
    { when: "told", userStartedOnly: false, title: { reply: words.finished(who), urgent: words.urgent(who) } },
    { when: "finished", userStartedOnly: true, title: words.finished(who), compacted: words.compacted },
  ];
  return call("configure", { configuration: { enabled, agentNames: { codex: "Codex", claude: "Claude" }, rules, troubles: troubleTemplates(words.troubles) } });
}

/** The words of each kind of trouble as the phone fills them in: a title
 *  and a detail, made from a trouble whose every fact is its place
 *  (`{machine}`, `{count}`, `{seconds}`, `{bound}`, `{used}`, `{version}`),
 *  and the bounds by their plain names. The hub's own near a limit is a
 *  kind of its own. */
function troubleTemplates(words: TroubleWords): { titles: Record<string, string>; details: Record<string, string>; bounds: Record<string, string> } {
  const held = { count: "{count}", seconds: "{seconds}", version: "{version}" } as unknown as Pick<Trouble, "count" | "seconds" | "version">;
  const near = { bound: "{bound}", used: "{used}" } as unknown as NonNullable<Trouble["near"]>;
  const of = (kind: Trouble["kind"], machine: boolean, nearing = false): Trouble => ({ id: 0, at: "", kind, ...held, ...(machine ? { machine: "{machine}" } : {}), ...(nearing ? { near } : {}) });
  const kinds: [string, Trouble][] = [
    ["silent", of("silent", true)],
    ["drops", of("drops", true)],
    ["stuck", of("stuck", true)],
    ["near", of("near", true, true)],
    ["damaged", of("damaged", true)],
    ["violations", of("violations", true)],
    ["other", of("other", true)],
    ["outage", of("outage", false)],
    ["hub", of("hub", false)],
    ["hub_near", of("hub", false, true)],
    ["rollout", of("rollout", false)],
  ];
  return {
    titles: Object.fromEntries(kinds.map(([key, trouble]) => [key, words.title(trouble)])),
    details: Object.fromEntries(kinds.map(([key, trouble]) => [key, words.detail(trouble)])),
    bounds: words.bounds,
  };
}

/** Tells the phone which conversation the page shows, none on another
 *  page: what that one says is not called about while the app is in front
 *  of the user. */
export function tellPhoneViewing(place: Place): Promise<void> {
  const conversation = place.kind === "talk" ? `${place.address.resident}@${place.address.machine}` : "";
  return call("viewing", { conversation });
}

interface Activation {
  id: string;
  route: string;
  action: "open";
  notificationId: string;
}

/** Opens what each tapped call was about, in the order they were tapped,
 *  each told back to the phone once it is open. The phone keeps them across
 *  the app being closed, so a cold start opens the last one too. */
export async function routeActivations(): Promise<void> {
  let draining = false;
  const drain = async () => {
    if (draining) return;
    draining = true;
    try {
      const { deliveries } = await call<{ deliveries: Activation[] }>("pending_activations");
      for (const delivery of deliveries) {
        location.hash = delivery.route;
        await call("acknowledge", { id: delivery.id });
      }
    } finally {
      draining = false;
    }
  };
  await listen("android-world-pending", () => void drain().catch((error: unknown) => console.error("opening a tapped call", error)));
  await drain();
}

export interface PhoneStatus {
  notificationsAllowed: boolean;
  alertChannelAllowed: boolean;
  batteryOptimizationExempt: boolean;
  powerSave: boolean;
  manufacturer: string;
  model: string;
  /** What the phone is called in its own settings, or its maker and model. */
  deviceName: string;
  /** Whether the background connection runs now, and why it could not start. */
  serviceRunning: boolean;
  serviceError: string | null;
  /** The World the phone is in, by its name; none before it joins one. */
  connection: { name: string | null };
  background: {
    vendor: "honor" | "huawei" | "xiaomi" | "other";
    backgroundRestricted: boolean | null;
    /** Whether the maker has a page where an app is let start by itself. */
    hasAutostart: boolean;
    /** How the app's last run ended, where Android keeps it. */
    lastExit: { reason: number; at: number; byUser: boolean } | null;
  };
}

export type Destination = "application" | "battery" | "notifications" | "autostart" | "vendorBattery";

/** What this phone remembers the user said: kept in the page's own storage,
 *  which a private window or a cleared phone may not keep. */
const remembered = (key: string): boolean => {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
};
const remember = (key: string): void => {
  try {
    localStorage.setItem(key, "1");
  } catch {
    // Not kept: asked again next time.
  }
};

/** The user said the maker's switches, which no app can read, are set. */
const VENDOR_SET = "phone-vendor-set";
/** The guide to the background was shown once. */
const GUIDE_SEEN = "phone-guide-seen";

/** Whether the system ended the app's last run in the background while it
 *  was meant to keep its connection: not the user, nor a crash of its own
 *  (`ApplicationExitInfo`: crash 4, native crash 5, not responding 6). */
export function killedInBackground(status: PhoneStatus): boolean {
  const exit = status.background.lastExit;
  return exit !== null && !exit.byUser && ![4, 5, 6].includes(exit.reason);
}

/** Whether anything the World needs to stay online in the background is
 *  not set, as far as it can tell. */
function unset(status: PhoneStatus): boolean {
  return (
    !status.notificationsAllowed ||
    !status.batteryOptimizationExempt ||
    !status.serviceRunning ||
    (status.background.hasAutostart && !remembered(VENDOR_SET))
  );
}

/** Once the phone is in a World, a sheet that walks the user through what
 *  keeps it online in the background — the system's own dialogs where the
 *  system has them, straight to the maker's page where it does not — shown
 *  once, and again when the system ended it in the background with
 *  something still unset. */
export function guideBackground(words: SettingsWords, phone: PhoneSeam): void {
  phone.status().then(
    (status) => {
      if (status.connection.name === null || !unset(status)) return;
      if (remembered(GUIDE_SEEN) && !killedInBackground(status)) return;
      remember(GUIDE_SEEN);
      const sheet = new Sheet("phone-guide");
      sheet.body.innerHTML = `${sheet.head(words.phoneGuideTitle, words.phoneGuideLater)}<p class="phone-guide-why">${escape(
        killedInBackground(status) ? words.phoneKilled : words.phoneGuideWhy,
      )}</p><dl class="facts settings-phone"></dl><footer class="sheet-foot"><span></span><button class="sheet-go sheet-close" type="button">${escape(words.phoneGuideDone)}</button></footer>`;
      new PhoneRow(sheet.body.querySelector<HTMLElement>(".settings-phone")!, words, phone);
      sheet.show();
    },
    (error: unknown) => console.error("reading what the phone lets the World do", error),
  );
}

/** What the settings ask of the phone. */
export interface PhoneSeam {
  status(): Promise<PhoneStatus>;
  /** Asks the user to allow calls, where the system asks. */
  allow(): Promise<unknown>;
  open(destination: Destination): Promise<unknown>;
}

export const phoneSeam: PhoneSeam = {
  status: () => call<PhoneStatus>("status"),
  allow: () => call("request_notifications"),
  open: (destination) => call("settings", { destination }),
};

/** The phone's own settings that decide whether the World can call the
 *  user from the background, as the phone reports them, with the way to
 *  change each. What a phone maker keeps private is not read; it is said
 *  where to find it. */
export class PhoneRow {
  private status: PhoneStatus | null = null;
  private said = "";
  /** What the phone or the World said, for the pointer's rest. */
  private why = "";

  constructor(
    private readonly element: HTMLElement,
    private readonly words: SettingsWords,
    private readonly phone: PhoneSeam,
  ) {
    element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-phone]");
      if (!button) return;
      const what = button.dataset.phone!;
      if (what === "vendorDone") {
        remember(VENDOR_SET);
        this.draw();
        return;
      }
      const done = what === "ask" ? this.phone.allow() : this.phone.open(what as Destination);
      void done.then(
        () => this.ask(),
        (error: unknown) => {
          this.why = String(error instanceof Error ? error.message : error);
          this.said = this.words.notDone(this.why);
          this.draw();
        },
      );
    });
    // Coming back from the phone's settings, what changed is read again.
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible") this.ask();
    });
    this.ask();
  }

  private ask(): void {
    this.phone.status().then(
      (status) => {
        this.status = status;
        this.draw();
      },
      (error: unknown) => {
        this.why = String(error instanceof Error ? error.message : error);
        this.said = this.words.notDone(this.why);
        this.draw();
      },
    );
  }

  private draw(): void {
    const { words } = this;
    const status = this.status;
    const button = (what: string, label: string) => `<button class="text-button quiet-button" type="button" data-phone="${what}">${escape(label)}</button>`;
    const rows: [string, string][] = [];
    if (status) {
      rows.push([
        words.phoneKeep,
        `<span>${escape(status.serviceRunning ? words.phoneKeepOn : words.phoneKeepOff)}</span>${
          killedInBackground(status) ? `<p class="settings-said">${escape(words.phoneKilled)}</p>` : ""
        }`,
      ]);
      const allowed = status.notificationsAllowed && status.alertChannelAllowed;
      rows.push([
        words.phoneNotices,
        `<span>${escape(allowed ? words.phoneNoticesOn : words.phoneNoticesOff)}</span>${allowed ? "" : status.notificationsAllowed ? button("notifications", words.phoneOpenNotices) : button("ask", words.phoneAllow)}`,
      ]);
      rows.push([
        words.phoneBattery,
        `<span>${escape(status.batteryOptimizationExempt ? words.phoneBatteryOn : words.phoneBatteryOff)}</span>${status.batteryOptimizationExempt ? "" : button("battery", words.phoneOpenBattery)}${
          status.powerSave ? `<p class="settings-said">${escape(words.phonePowerSave)}</p>` : ""
        }`,
      ]);
      const steps = words.phoneVendor[status.background.vendor];
      if (steps.length) {
        // The maker's own switches no app can read: straight to their page,
        // and the user's word that they are set.
        const set = remembered(VENDOR_SET);
        rows.push([
          words.phoneMaker(status.manufacturer),
          `<ol class="settings-steps">${steps.map((step) => `<li>${escape(step)}</li>`).join("")}</ol>${
            status.background.hasAutostart ? button("autostart", words.phoneAutostart) : ""
          }${status.background.vendor === "xiaomi" ? button("vendorBattery", words.phoneVendorBattery) : ""}${button("application", words.phoneOpenApp)}${
            set ? `<span>${escape(words.phoneVendorSet)}</span>` : button("vendorDone", words.phoneVendorDone)
          }`,
        ]);
      }
    }
    this.element.hidden = !rows.length && !this.said;
    this.element.innerHTML =
      rows.map(([label, body]) => `<dt>${escape(label)}</dt><dd>${body}</dd>`).join("") +
      (this.said ? `<dt></dt><dd><p class="settings-said" role="status"${this.why ? ` title="${escape(this.why)}"` : ""}>${escape(this.said)}</p></dd>` : "");
  }
}
