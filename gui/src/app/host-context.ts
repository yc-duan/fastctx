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
// Codex: the context is `model_context_window`, in tokens, held to the most
// the model may be set to; when it compacts is
// `model_auto_compact_token_limit`, in tokens, never past nine tenths of the
// context, where it compacts when none is set.
//
// Each model's window and its most come with the models a node lists
// (`AgentModel`): the steps run to the most the model chosen, or the login's
// default, may take, and the knob rests on its own window.

import type { EffortStep } from "../conversation/effort-slider";
import type { AgentModel } from "../world/agents";
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

/** What a model's window is taken to be where its node does not say: a
 *  Claude model of this generation's, and what Codex's service listed. */
const CLAUDE_WINDOW = 1_000_000;
const CODEX_WINDOW = 272_000;
const CODEX_MOST = 872_000;
/** The rounder sizes a slider steps through, below a model's most. */
const CLAUDE_SIZES = [200_000, 400_000, 600_000, 800_000];
const CODEX_SIZES = [200_000, 400_000, 600_000, 800_000];
/** Claude Code takes no window below this. */
const CLAUDE_LEAST = 100_000;
/** What Claude Code keeps of a window for the answer, and below that for
 *  the summary, where it compacts when nothing says. */
const CLAUDE_ANSWER = 20_000;
const CLAUDE_SUMMARY = 13_000;
const CLAUDE_PCT = "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE";
/** Where Codex compacts unless told: nine tenths of its window. */
const CODEX_COMPACTS_AT = 0.9;
const SHARES = [50, 60, 70, 80, 90];
/** The step past the last share: no compacting by itself. */
const NEVER = "never";

export function hostContext(agent: "claude" | "codex", english: boolean, p: ProfileWords["prefs"], model: (table: Table) => AgentModel | null): HostContext {
  const size = (tokens: number) => tokenSize(tokens, english);
  const share = (percent: number) => `${percent}%`;
  /** The model's own window, and the most it may be set to. */
  const own = (table: Table) => {
    const chosen = model(table);
    const window = chosen?.context_window ?? (agent === "claude" ? CLAUDE_WINDOW : CODEX_WINDOW);
    const most = Math.max(window, chosen?.max_context_window ?? (agent === "claude" ? window : CODEX_MOST));
    return { window, most };
  };
  /** The sizes to step through: the rounder ones below the most, the
   *  model's own window and the most itself. */
  const sizes = (table: Table) => {
    const { window, most } = own(table);
    const rounder = (agent === "claude" ? CLAUDE_SIZES : CODEX_SIZES).filter((tokens) => tokens < most);
    const least = agent === "claude" && most <= CLAUDE_SIZES[0]! ? [CLAUDE_LEAST] : [];
    return [...new Set([...least, ...rounder, window, most])].filter((tokens) => agent !== "claude" || tokens >= CLAUDE_LEAST).sort((a, b) => a - b);
  };
  const windowSteps = (table: Table) => sizes(table).map((tokens) => ({ value: String(tokens), label: size(tokens) }));
  const windowResting = (table: Table) => {
    const { window } = own(table);
    return { at: Math.max(0, sizes(table).indexOf(window)), label: size(window) };
  };
  if (agent === "claude") {
    const windowOf = (table: Table) => (typeof table.autoCompactWindow === "number" ? Math.min(table.autoCompactWindow, own(table).most) : own(table).window);
    const env = (table: Table): Table => (isTable(table.env) ? table.env : {});
    const shareSteps = [...SHARES.map((percent) => ({ value: String(percent), label: share(percent) })), { value: NEVER, label: p.never }];
    return {
      window: {
        kind: "slider",
        id: "window",
        label: p.window,
        steps: windowSteps,
        get: (table) => (typeof table.autoCompactWindow === "number" ? String(table.autoCompactWindow) : null),
        set: (table, value) => withKey(table, "autoCompactWindow", value === null ? undefined : Number(value)),
        resting: windowResting,
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
  const windowOf = (table: Table) => (typeof table.model_context_window === "number" ? Math.min(table.model_context_window, own(table).most) : own(table).window);
  const limitOf = (table: Table) => (typeof table.model_auto_compact_token_limit === "number" ? table.model_auto_compact_token_limit : null);
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
      steps: windowSteps,
      get: (table) => (typeof table.model_context_window === "number" ? String(table.model_context_window) : null),
      set: (table, value) => {
        // The compaction point keeps its share of the new window.
        const kept = shareOf(table);
        const next = withKey(table, "model_context_window", value === null ? undefined : Number(value));
        return kept === null ? next : withKey(next, "model_auto_compact_token_limit", Math.round((windowOf(next) * kept) / 100));
      },
      resting: windowResting,
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
