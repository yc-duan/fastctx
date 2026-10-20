// How far an agent's context grows and when it is compacted, as each host
// keeps them (`world-user-config.md`, 偏好). Both are sliders of steps on the
// agent's preferences (`./host-prefs`); the compaction point is a share of
// the context, so changing the context keeps it where it stood.
//
// Claude Code (2.1.303): the context a session grows to is
// `autoCompactWindow`, in tokens, from 100 thousand to a million, held to
// the model's own window, which it is left alone. Of it, 20 thousand are
// kept for the answer; the rest less 13 thousand is where it compacts.
// `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`, set in the settings' `env`, brings that
// earlier, to a share of the window less the answer's part; past the last
// step it does not compact by itself (`autoCompactEnabled`).
//
// Codex: the context is `model_context_window`, in tokens, held to what the
// model can take; when it compacts is `model_auto_compact_token_limit`, in
// tokens, never past nine tenths of the context, where it compacts when
// none is set. Its models' windows are not listed, so the window every
// listed model has stands for the default.

import type { EffortStep } from "../conversation/effort-slider";
import type { ProfileWords } from "./profile-words";

type Table = Record<string, unknown>;

/** The rows the preferences lay out; see `Slid` in `./host-prefs`. */
export interface ContextRow {
  kind: "slider";
  id: string;
  label: string;
  steps(table: Table): EffortStep[];
  get(table: Table): string | null;
  set(table: Table, value: string | null): Table;
  resting(table: Table): { at: number; label: string };
  tip(table: Table): string;
}

export interface HostContext {
  window: ContextRow;
  compact: ContextRow;
}

/** Claude Code's window for the models of this generation. */
const CLAUDE_WINDOW = 1_000_000;
const CLAUDE_WINDOWS = [200_000, 400_000, 600_000, 800_000, 1_000_000];
/** What Claude Code keeps of a window for the answer, and below that for
 *  the summary, where it compacts when nothing says. */
const CLAUDE_ANSWER = 20_000;
const CLAUDE_SUMMARY = 13_000;
const CLAUDE_PCT = "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE";
/** Codex's window for every model it lists, and where it compacts unless
 *  told: nine tenths of it. */
const CODEX_WINDOW = 272_000;
const CODEX_WINDOWS = [128_000, 200_000, 272_000];
const CODEX_COMPACTS_AT = 0.9;
const SHARES = [50, 60, 70, 80, 90];
/** The step past the last share: no compacting by itself. */
const NEVER = "never";

export function hostContext(agent: "claude" | "codex", english: boolean, p: ProfileWords["prefs"]): HostContext {
  const size = (tokens: number) => tokenSize(tokens, english);
  const share = (percent: number) => `${percent}%`;
  if (agent === "claude") {
    const windowOf = (table: Table) => (typeof table.autoCompactWindow === "number" ? table.autoCompactWindow : CLAUDE_WINDOW);
    const env = (table: Table): Table => (isTable(table.env) ? table.env : {});
    const windowSteps = CLAUDE_WINDOWS.map((tokens) => ({ value: String(tokens), label: size(tokens) }));
    const shareSteps = [...SHARES.map((percent) => ({ value: String(percent), label: share(percent) })), { value: NEVER, label: p.never }];
    return {
      window: {
        kind: "slider",
        id: "window",
        label: p.window,
        steps: () => windowSteps,
        get: (table) => (typeof table.autoCompactWindow === "number" ? String(table.autoCompactWindow) : null),
        set: (table, value) => withKey(table, "autoCompactWindow", value === null ? undefined : Number(value)),
        resting: () => ({ at: CLAUDE_WINDOWS.indexOf(CLAUDE_WINDOW), label: size(CLAUDE_WINDOW) }),
        tip: (table) => p.windowTip(size(windowOf(table))),
      },
      compact: {
        kind: "slider",
        id: "compact",
        label: p.compact,
        steps: () => shareSteps,
        get: (table) => {
          if (table.autoCompactEnabled === false) return NEVER;
          const percent = env(table)[CLAUDE_PCT];
          return typeof percent === "string" && SHARES.includes(Number(percent)) ? percent : null;
        },
        set: (table, value) => {
          const rest = { ...env(table) };
          delete rest[CLAUDE_PCT];
          if (value !== null && value !== NEVER) rest[CLAUDE_PCT] = value;
          const next = withKey(table, "env", Object.keys(rest).length ? rest : undefined);
          return withKey(next, "autoCompactEnabled", value === NEVER ? false : undefined);
        },
        resting: (table) => restingShare(claudeCompacts(windowOf(table), null) / windowOf(table), share),
        tip: (table) => {
          if (table.autoCompactEnabled === false) return p.neverTip;
          const percent = env(table)[CLAUDE_PCT];
          return p.compactTip(size(claudeCompacts(windowOf(table), typeof percent === "string" ? Number(percent) : null)));
        },
      },
    };
  }
  const windowOf = (table: Table) => (typeof table.model_context_window === "number" ? table.model_context_window : CODEX_WINDOW);
  const limitOf = (table: Table) => (typeof table.model_auto_compact_token_limit === "number" ? table.model_auto_compact_token_limit : null);
  const windowSteps = CODEX_WINDOWS.map((tokens) => ({ value: String(tokens), label: size(tokens) }));
  const shareSteps = SHARES.map((percent) => ({ value: String(percent), label: share(percent) }));
  /** The share of the window a limit stands at, as the nearest step. */
  const shareOf = (table: Table): number | null => {
    const limit = limitOf(table);
    if (limit === null) return null;
    const percent = (limit / windowOf(table)) * 100;
    return SHARES.reduce((best, step) => (Math.abs(step - percent) < Math.abs(best - percent) ? step : best));
  };
  return {
    window: {
      kind: "slider",
      id: "window",
      label: p.window,
      steps: () => windowSteps,
      get: (table) => (typeof table.model_context_window === "number" ? String(table.model_context_window) : null),
      set: (table, value) => {
        // The compaction point keeps its share of the new window.
        const kept = shareOf(table);
        const next = withKey(table, "model_context_window", value === null ? undefined : Number(value));
        return kept === null ? next : withKey(next, "model_auto_compact_token_limit", Math.round((windowOf(next) * kept) / 100));
      },
      resting: () => ({ at: CODEX_WINDOWS.indexOf(CODEX_WINDOW), label: size(CODEX_WINDOW) }),
      tip: (table) => p.windowTip(size(windowOf(table))),
    },
    compact: {
      kind: "slider",
      id: "compact",
      label: p.compact,
      steps: () => shareSteps,
      get: (table) => {
        const percent = shareOf(table);
        return percent === null ? null : String(percent);
      },
      set: (table, value) => withKey(table, "model_auto_compact_token_limit", value === null ? undefined : Math.round((windowOf(table) * Number(value)) / 100)),
      resting: () => restingShare(CODEX_COMPACTS_AT, share),
      tip: (table) => p.compactTip(size(limitOf(table) ?? Math.round(windowOf(table) * CODEX_COMPACTS_AT))),
    },
  };
}

/** Where Claude Code compacts in a window, with a share set or none. */
function claudeCompacts(window: number, percent: number | null): number {
  const room = window - CLAUDE_ANSWER;
  const standard = room - CLAUDE_SUMMARY;
  return percent === null ? standard : Math.min(Math.floor((room * percent) / 100), standard);
}

/** Where the knob rests for a default share: on its step, or between the
 *  two it falls between. */
function restingShare(at: number, share: (percent: number) => string): { at: number; label: string } {
  const percent = Math.round(at * 100);
  const last = SHARES.length - 1;
  const step = SHARES.findIndex((one) => one >= percent);
  if (step === -1) return { at: Math.min(last + (percent - SHARES[last]!) / 10, last + 0.9), label: share(percent) };
  if (SHARES[step] === percent) return { at: step, label: share(percent) };
  const below = SHARES[step - 1] ?? 0;
  return { at: step - 1 + (percent - below) / (SHARES[step]! - below), label: share(percent) };
}

/** Tokens as people say them: 「20 万」, 「27.2 万」, 「100 万」; 200K, 1M. */
export function tokenSize(tokens: number, english: boolean): string {
  if (english) return tokens >= 1_000_000 ? `${trim(tokens / 1_000_000)}M` : `${trim(tokens / 1000)}K`;
  return `${trim(tokens / 10_000)} 万`;
}

function trim(value: number): string {
  return String(Math.round(value * 10) / 10);
}

function isTable(value: unknown): value is Table {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function withKey(table: Table, key: string, value: unknown): Table {
  const next = { ...table };
  if (value === undefined) delete next[key];
  else next[key] = value;
  return next;
}
