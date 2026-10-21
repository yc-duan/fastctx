// What a plain browser shows of a login's requests: thirty days of turns
// for those its specimen usage lists, made up but shaped like real use: a
// turn is a few requests, its context read from cache growing as it goes,
// now and then a subagent's on a smaller model, Claude Code's own
// background requests among them, and a few that failed, were stopped or
// were held at an allowance. The reader's own conversations are named; the
// others' are not, as the login's machine answers.

import type { HubAccount } from "../world/accounts";
import type { CallOutcome, UsageCall, UsageCallsPage, UsageCallsQuery } from "../world/usage-calls";
import { specimenUsage } from "./usage";

const MINUTE = 60_000;
const DAY = 86_400_000;
const PAGE = 100;

const made = new Map<string, Promise<UsageCall[]>>();

export async function specimenCalls(machine: string, agent: string, dir: string, query: UsageCallsQuery, at: Date): Promise<UsageCallsPage> {
  const key = `${machine}|${agent}|${dir}`;
  let rows = made.get(key);
  if (!rows) made.set(key, (rows = make(machine, agent, dir, at)));
  const all = await rows;
  const person = query.person;
  const matching = all.filter(
    (row) =>
      (!person || (person.account === null ? row.who === null : row.who?.hub === person.account.hub && row.who.account === person.account.account)) &&
      (!query.model || row.model === query.model) &&
      (!query.source || row.source === query.source) &&
      (!query.outcome || row.outcome === query.outcome) &&
      (!query.conversation || (row.conversation === query.conversation && (!query.turn || row.turn === query.turn))),
  );
  const start = query.before ? matching.findIndex((row) => Number(row.cursor.key) > Number(query.before!.key)) : 0;
  const from = start < 0 ? matching.length : start;
  const page = matching.slice(from, from + PAGE);
  await new Promise((resolve) => setTimeout(resolve, 260));
  return { rows: page, next: from + PAGE < matching.length ? page[page.length - 1]!.cursor : null };
}

async function make(machine: string, agent: string, dir: string, at: Date): Promise<UsageCall[]> {
  const report = await specimenUsage(machine, agent, dir, at);
  const claude = agent === "claude";
  const random = seeded(`calls:${machine}|${agent}|${dir}`);
  const people = report.people.map((one) => one.account);
  const mine = (who: HubAccount | null) => who === null || who.account === "me" || who.account === "yc";
  const rows: UsageCall[] = [];
  const now = at.getTime();
  let index = 0;
  // Turns walk back from now, a few minutes to an hour or two apart in the
  // day, the nights mostly quiet.
  for (let end = now - 2 * MINUTE; end > now - 30 * DAY; ) {
    const hour = new Date(end).getHours();
    if (hour < 7 && random() < 0.9) {
      end -= 40 * MINUTE;
      continue;
    }
    const who = people[Math.floor(random() ** 1.6 * people.length)] ?? null;
    const own = mine(who);
    const conversation = own ? (claude ? "claude-5teps" : "codex-a51c") : null;
    const turn = own ? `t-${Math.floor(end / MINUTE)}` : null;
    const steps = 1 + Math.floor(random() * random() * 14);
    let context = 18_000 + random() * 60_000;
    let at = end;
    for (let step = 0; step < steps; step += 1) {
      const agentStep = claude && step > 1 && random() < 0.22;
      const model = claude ? (agentStep ? (random() < 0.7 ? "claude-haiku-5-5" : "claude-sonnet-5-5") : random() < 0.08 ? "claude-fable-5-1" : "claude-opus-5-5") : random() < 0.8 ? "gpt-6-luna" : "gpt-6";
      const output = Math.round(80 + random() ** 2 * 4200);
      const input = Math.round(400 + random() * 2600);
      const write = claude ? Math.round(random() < 0.3 ? random() * 24_000 : random() * 1800) : 0;
      const read = Math.round(agentStep ? context * 0.25 : context);
      const reasoning = claude ? 0 : Math.round(random() < 0.5 ? random() * 1800 : 0);
      const took = 900 + random() * (output / 30) * 1000;
      const first = 400 + random() * 1800;
      const roll = random();
      const outcome: CallOutcome = roll < 0.012 ? "failed" : roll < 0.022 ? "interrupted" : claude && roll < 0.027 ? "held" : "completed";
      const sent = outcome !== "held";
      // Millionths of a dollar, as the ledger keeps them.
      const usd = claude ? (sent ? Math.round((model.includes("haiku") ? 0.2 : model.includes("sonnet") ? 0.6 : 1) * (input * 5 + write * 6.25 + read * 0.5 + output * 25)) : 0) : null;
      rows.push({
        cursor: { at: new Date(at).toISOString(), key: "" },
        who,
        model,
        tokens: sent ? (claude ? { input, cache_read: read, cache_write: write, output } : { input, cache_read: read, output, reasoning }) : null,
        usd,
        source: agentStep ? "agent" : "main",
        agent: agentStep ? `a${Math.floor(random() * 1e8).toString(16)}` : null,
        timing: { started_at: new Date(at - took).toISOString(), first_at: claude && sent ? new Date(at - took + first).toISOString() : null, ended_at: new Date(at).toISOString(), approximate: !claude },
        outcome,
        phase: outcome === "failed" ? "before_response" : outcome === "interrupted" ? "streaming" : outcome === "held" ? "before_response" : "finished",
        conversation,
        turn,
      });
      context += input + output + write * 0.4;
      at -= took + 300 + random() * 4000;
      // Now and then Claude Code's own request between two of the turn's.
      if (claude && random() < 0.05) {
        rows.push({
          cursor: { at: new Date(at).toISOString(), key: "" },
          who,
          model: null,
          tokens: null,
          usd: Math.round((0.002 + random() * 0.03) * 1_000_000),
          source: "background",
          agent: null,
          timing: { started_at: null, first_at: null, ended_at: new Date(at).toISOString(), approximate: false },
          outcome: "unknown",
          phase: "unknown",
          conversation,
          turn: null,
        });
        at -= 800;
      }
    }
    end = at - (2 + random() ** 2 * 90) * MINUTE;
  }
  for (const row of rows) row.cursor.key = String(index++);
  return rows;
}

function seeded(seed: string): () => number {
  let state = 2166136261;
  for (const char of seed) state = Math.imul(state ^ char.charCodeAt(0), 16777619);
  return () => {
    state = Math.imul(state ^ (state >>> 15), 2246822507);
    state = Math.imul(state ^ (state >>> 13), 3266489909);
    state ^= state >>> 16;
    return (state >>> 0) / 4294967296;
  };
}
