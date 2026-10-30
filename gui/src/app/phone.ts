// The interface on an Android phone, beside the page: the phone calls the
// user when an agent starts waiting for them or a conversation they opened
// finishes, even with the app in the background; a tap on that call opens
// the conversation; and the settings say what the phone lets the World do
// in the background and where to change it. Watching who is waiting runs in
// the app's own process, without the page (`src-tauri/src/android`); the
// page gives it the rules and their words, the same as on a computer
// (`presence.ts`), and routes the taps.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inShell } from "../world/bridge";
import { escape } from "../conversation/prose";
import type { Place } from "./places";
import type { PresenceWords } from "./presence";
import type { SettingsWords } from "./words";

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
 *  asks the user something, one that tells them a reply or something
 *  urgent (what can wait is not pushed), and a turn the user's words began
 *  in a conversation opened through the World that ends without telling
 *  them anything. The phone writes the body from the agent's own words, as
 *  a computer does. */
export function configurePhone(words: PresenceWords, enabled: boolean): Promise<void> {
  const who = "{who}";
  const rules: Rule[] = [
    { when: "asked", userStartedOnly: false, title: Object.fromEntries(ASKED_FOR.map((what) => [what, words.asked(who, what)])) },
    { when: "told", userStartedOnly: true, title: { reply: words.finished(who), urgent: words.urgent(who) } },
    { when: "finished", userStartedOnly: true, title: words.finished(who), compacted: words.compacted },
  ];
  return call("configure", { configuration: { enabled, agentNames: { codex: "Codex", claude: "Claude" }, rules } });
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
  background: { vendor: "honor" | "xiaomi" | "other"; backgroundRestricted: boolean | null };
}

export type Destination = "application" | "battery" | "notifications";

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
        rows.push([
          words.phoneMaker(status.manufacturer),
          `<ol class="settings-steps">${steps.map((step) => `<li>${escape(step)}</li>`).join("")}</ol>${button("application", words.phoneOpenApp)}`,
        ]);
      }
    }
    this.element.hidden = !rows.length && !this.said;
    this.element.innerHTML =
      rows.map(([label, body]) => `<dt>${escape(label)}</dt><dd>${body}</dd>`).join("") +
      (this.said ? `<dt></dt><dd><p class="settings-said" role="status"${this.why ? ` title="${escape(this.why)}"` : ""}>${escape(this.said)}</p></dd>` : "");
  }
}
