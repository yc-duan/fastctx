/** Why a direct link is down, in the reader's words: the node says it as
 *  each try's address, route and outcome (`fastctx-net` `dial`), or as the
 *  way a link that was up ended. */

export interface DirectWords {
  /** Connected, then nothing within `seconds`. */
  silent(seconds: number): string;
  silentShort: string;
  /** No connection within `seconds`. */
  unreachable(seconds: number): string;
  unreachableShort: string;
  refused: string;
  failed: string;
  noAddress: string;
  cut: string;
  /** Nothing came over the link for `seconds`. */
  quiet(seconds: number): string;
  quietShort: string;
  closed: string;
  /** The routes a try took. */
  card(name: string): string;
  system: string;
  proxy(at: string): string;
  here: string;
  /** One try, as a line of the details. */
  attempt(address: string, route: string, outcome: string): string;
}

/** What is shown on the row, and every try where the pointer rests. */
export interface DirectWhy {
  short: string;
  detail: string;
}

interface Outcome {
  short: string;
  long: string;
}

const TRY = /^(\S+) (directly through "(.*)"|along the system's routes|through (\S+)|on this machine): (.+)$/;

/** How `raw` says why the link is down, or nothing when it says nothing. */
export function directWhy(raw: string | undefined, words: DirectWords): DirectWhy | null {
  const text = raw?.trim();
  if (!text) return null;
  const tries = text.split("; ").map((part) => TRY.exec(part));
  if (tries.every((found) => found !== null)) {
    const outcomes = tries.map((found) => outcome(found![5]!, words));
    const lines = tries.map((found, index) => {
      const [, address, , card, proxy] = found!;
      const route = card !== undefined ? words.card(card) : proxy !== undefined ? words.proxy(proxy) : found![2] === "on this machine" ? words.here : words.system;
      return words.attempt(address!, route, outcomes[index]!.long);
    });
    return { short: outcomes[0]!.short, detail: lines.join("\n") };
  }
  const said = outcome(text, words);
  return { short: said.short, detail: said.long === said.short ? "" : said.long };
}

function outcome(text: string, words: DirectWords): Outcome {
  let found = /^connected, then no answer within (\d+) s$/.exec(text);
  if (found) return { short: words.silentShort, long: words.silent(Number(found[1])) };
  found = /^no connection within (\d+) s$/.exec(text);
  if (found) return { short: words.unreachableShort, long: words.unreachable(Number(found[1])) };
  found = /nothing came from it for (\d+) s/.exec(text);
  if (found) return { short: words.quietShort, long: words.quiet(Number(found[1])) };
  if (/refused/i.test(text)) return { short: words.refused, long: `${words.refused}（${text}）` };
  if (text === "no address is known") return { short: words.noAddress, long: words.noAddress };
  if (text === "the connection was cut") return { short: words.cut, long: words.cut };
  if (/closed the link/.test(text)) return { short: words.closed, long: words.closed };
  return { short: words.failed, long: text };
}
