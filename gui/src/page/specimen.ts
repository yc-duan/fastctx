// A stand-in for the host, so a page can be looked at in a plain browser:
// the World's tools answer with made-up but plausible facts, the doing
// tools after a moment. Loaded before the runtime by `page-specimen.html`.

import type { Bridge, PageContext, Theme } from "./bridge";
import { answers } from "./specimen-answers";

const params = new URLSearchParams(location.search);
const context: PageContext = {
  theme: (params.get("theme") as Theme | null) ?? "warm-black",
  locale: params.get("lang") === "en" ? "en" : "zh-CN",
};

const bridge: Bridge = {
  context,
  onContext: () => () => {},
  async call<T>(tool: string, args: Record<string, unknown> = {}): Promise<T> {
    const answer = answers[tool];
    if (!answer) throw new Error(`this specimen has no ${tool}`);
    await new Promise((resolve) => setTimeout(resolve, tool === "run" || tool === "say" ? 900 : 120));
    return answer(args) as T;
  },
};

window.__worldBridge = bridge;
