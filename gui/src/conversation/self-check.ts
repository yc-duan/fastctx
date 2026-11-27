// A conversation's page checks what it shows against its own records once
// the conversation rests (`Conversation.selfCheck`), as the bench checks
// recordings: what it finds is counted on this computer by cause, never by
// words, and the first time a cause is found, what the page was given is
// kept beside the application's data for the bench to replay. How many
// checks ran is counted too, so that none running shows.

import type { Address } from "../world/events";
import type { Conversation } from "./conversation";
import type { Tape } from "./tape";

/** How long a conversation rests before its page checks itself. */
const REST_MS = 4_000;
/** The most causes counted, and the rows kept of each. */
const CAUSES_AT_MOST = 50;
const STORE = "fastctx.conversation-checks.v1";

interface Counted {
  /** Checks run, and when the count began. */
  runs: number;
  since: string;
  found: Record<string, { count: number; last: string; rows: unknown[] }>;
}

function load(): Counted {
  try {
    const kept = JSON.parse(localStorage.getItem(STORE) ?? "null") as Counted | null;
    if (kept && typeof kept.runs === "number" && kept.found) return kept;
  } catch {
    // Unreadable or unavailable: counted anew.
  }
  return { runs: 0, since: new Date().toISOString(), found: {} };
}

function save(counted: Counted): void {
  try {
    localStorage.setItem(STORE, JSON.stringify(counted));
  } catch {
    // A full or unavailable store keeps no count; the page goes on.
  }
}

/** Checks one page's conversation whenever it comes to rest. `keep` keeps
 *  `tape`, the recording of what the page was given, where the shell can. */
export class SelfChecks {
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly address: Address,
    private readonly conversation: Conversation,
    private readonly tape: Tape | null,
    private readonly keep: ((name: string, text: string) => Promise<void>) | null,
  ) {}

  /** Something came: the check waits for the conversation to rest. */
  poke(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      this.run();
    }, REST_MS);
  }

  stop(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  private run(): void {
    let found;
    try {
      found = this.conversation.selfCheck(Date.now());
    } catch (error) {
      found = { kind: "differs" as const, fingerprint: `threw:${error instanceof Error ? error.name : "error"}`, rows: [] };
    }
    const counted = load();
    counted.runs += 1;
    if (found) {
      const now = new Date().toISOString();
      const held = counted.found[found.fingerprint];
      counted.found[found.fingerprint] = { count: (held?.count ?? 0) + 1, last: now, rows: found.rows };
      const causes = Object.entries(counted.found);
      if (causes.length > CAUSES_AT_MOST) {
        causes.sort((a, b) => a[1].last.localeCompare(b[1].last));
        delete counted.found[causes[0]![0]];
      }
      if (!held && this.keep && this.tape) {
        const { text, whole } = this.tape.read();
        const name = `${now.replace(/[:.]/g, "-")}-${this.address.resident}${whole ? "" : "-partial"}.jsonl`;
        void this.keep(name, text).catch(() => {});
      }
    }
    save(counted);
  }
}
