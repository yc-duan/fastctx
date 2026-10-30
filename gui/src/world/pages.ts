// Pages as the World keeps them (`fastctx_types::pages`): each version of a
// page is an entry in its project's ledger, and so is the user's consent to
// what a page may do. The interface lists them, opens one, asks the user's
// consent, and carries out what an open page asks, through the core, which
// checks every act against the version's declaration and the consent.

import { invoke } from "@tauri-apps/api/core";
import type { Address, MachineName, TownName } from "./wire";
import type { Stamp, Writer } from "./world";

export type PageName = string;

/** One thing a page may do, compared literally across declaration and
 *  consent: a whole command on a machine, words to one resident, entries in
 *  one journal. */
export type PageAct =
  | { kind: "run"; machine: MachineName; script: string }
  | { kind: "say"; to: Address }
  | { kind: "journal"; journal: string };

export interface PageVersion {
  version: number;
  base: number | null;
  title: string;
  description: string;
  acts: PageAct[];
  /** Taken down: this version has no HTML, and the page is not shown. */
  withdrawn: boolean;
  /** Brought back: this version is a copy of that one. */
  restored_from: number | null;
  by: PageWriter;
  /** Left out as a project record's is. */
  on?: MachineName;
  at: string;
}

/** Who wrote a version: anyone who writes into a ledger, a page too. */
export type PageWriter = Writer;

/** What the core stamps on whatever an open page did. */
export interface PageSource {
  town: TownName;
  page: PageName;
  version: number;
  by: PageWriter;
  on: MachineName;
  opened_on: MachineName;
}

export interface PageConsent {
  version: number;
  base: number | null;
  /** The page's version the user was looking at when they agreed. */
  page_version: number;
  acts: PageAct[];
  stamp: Stamp;
}

export interface PageSummary {
  page: PageName;
  current: PageVersion;
}

export interface PageView {
  town: TownName;
  page: PageName;
  publication: PageVersion;
  html: string | null;
  /** The consent as it stands now, whichever version was read. */
  consent: PageConsent | null;
}

export type PageInvocation =
  | { kind: "run"; machine: MachineName; script: string }
  | { kind: "say"; to: Address; text: string }
  | { kind: "journal"; journal: string; text: string };

export type ExitStatus = { code: number } | { signal: number };

export type RunState =
  | { kind: "ended"; status: ExitStatus }
  | { kind: "exited"; status: ExitStatus; remaining: number }
  | { kind: "running" }
  | { kind: "stopped" }
  | { kind: "interrupted"; reason: string }
  | { kind: "failed"; error: string };

export type OutputPiece =
  | { kind: "text"; first_line: number; text: string }
  | { kind: "part_of_line"; line: number; from: number; to: number; of: number; text: string }
  | { kind: "omitted"; first_line: number; last_line: number }
  | { kind: "image"; mime: string; width: number; height: number; base64: string }
  | { kind: "unreadable"; line: number; reason: string };

export interface RunReply {
  run?: import("./run-facts").RunFacts;
  machine: MachineName;
  job: string;
  state: RunState;
  output: {
    log: string;
    bytes: number;
    lines: number;
    pieces: OutputPiece[];
    /** Lines shown that were not UTF-8 but decoded in the machine's legacy
     *  code page. */
    legacy: { encoding: string; lines: number }[];
    /** Lines holding bytes no known encoding decodes. */
    escaped_lines: number;
  };
  /** The private places macOS refused Fastctx there, as its node found. */
  privacy?: string[];
}

export type PageResult =
  | ({ kind: "run" } & RunReply)
  | { kind: "said"; to: Address; landed: unknown }
  | { kind: "entry_added"; journal: string; id: string };

/** Whether `acts` holds `act`, compared as the core compares them. */
export function holds(acts: PageAct[], act: PageAct): boolean {
  return acts.some((held) => sameAct(held, act));
}

export function sameAct(a: PageAct, b: PageAct): boolean {
  if (a.kind === "run" && b.kind === "run") return a.machine === b.machine && a.script === b.script;
  if (a.kind === "say" && b.kind === "say") return a.to.resident === b.to.resident && a.to.machine === b.to.machine;
  if (a.kind === "journal" && b.kind === "journal") return a.journal === b.journal;
  return false;
}

/** What a version declares that the user has not agreed to. */
export function unconsented(page: PageView): PageAct[] {
  const agreed = page.consent?.acts ?? [];
  return page.publication.acts.filter((act) => !holds(agreed, act));
}

/** Reads the page as `account` sees it, the one of this device's logins
 *  the project is for; none where this device has one login there. */
export function readPage(town: TownName, page: PageName, version: number | null, account: string | null = null): Promise<PageView> {
  return invoke<PageView>("page_read", { town, page, version, account });
}

export function pageHistory(town: TownName, page: PageName): Promise<PageVersion[]> {
  return invoke<PageVersion[]>("page_history", { town, page });
}

/** Sets what the user agrees the page may do, as `account`, replacing
 *  what they agreed before; `base` is the consent they were shown. What a
 *  page does is done for the account that agreed to it. */
export function consent(town: TownName, page: PageName, version: number, base: number | null, acts: PageAct[], account: string | null = null): Promise<PageConsent> {
  return invoke<PageConsent>("page_consent", { town, page, version, base, acts, account });
}

export function act(town: TownName, page: PageName, version: number, action: PageInvocation, account: string | null = null): Promise<PageResult> {
  return invoke<PageResult>("page_act", { town, page, version, action, account });
}

/** Takes the page down: a new version without HTML. */
export function withdraw(town: TownName, page: PageName, base: number): Promise<PageView> {
  return invoke<PageView>("page_withdraw", { town, page, base });
}

/** Brings `version` back as a new version. */
export function restore(town: TownName, page: PageName, base: number, version: number): Promise<PageView> {
  return invoke<PageView>("page_restore", { town, page, base, version });
}
