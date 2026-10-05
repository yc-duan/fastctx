// Words in a page: Markdown read as the conversation reads an agent's prose
// (code, tables, math, the same faces), and code highlighted as there.
//
//   <w-markdown><script type="text/markdown"># 结论 …</script></w-markdown>
//   <w-code lang="python"><script type="text/plain">print("hi")</script></w-code>
//
// The text is a script inside the element (so the page's HTML does not
// read it as markup), the element's own text, data a script set, or what a
// bound tool answered.

import hljs from "highlight.js/lib/common";
import base from "../../design/base.css?raw";
import highlight from "../../design/highlight.css?raw";
import { render } from "../../conversation/prose";
import { BOUND, PageElement, escape, sheet } from "../element";
import { themedWithin } from "../themed";

const prose = sheet(`${base}\n${themedWithin(highlight)}\n:host { display: block; }`);

/** The text an element holds, from wherever it was given. */
function textOf(element: PageElement & HTMLElement, data: unknown): string {
  if (typeof data === "string") return data;
  const script = element.querySelector<HTMLScriptElement>('script[type="text/markdown"], script[type="text/plain"]');
  return (script?.textContent ?? element.textContent ?? "").replace(/^\n+|\s+$/g, "");
}

function failed(data: unknown): string | null {
  return data && typeof data === "object" && "error" in data && typeof (data as { error: unknown }).error === "string" ? (data as { error: string }).error : null;
}

export class Markdown extends PageElement {
  static observedAttributes = [...BOUND];

  constructor() {
    super();
    this.shadow.adoptedStyleSheets = [...this.shadow.adoptedStyleSheets, prose];
  }

  protected draw(): void {
    if (this.loading) {
      this.shadow.innerHTML = `<p class="faint">…</p>`;
      return;
    }
    const data = this.data;
    const wrong = failed(data);
    this.shadow.innerHTML = wrong ? `<p class="wrong">${escape(wrong)}</p>` : `<div class="prose">${render(textOf(this, data))}</div>`;
  }
}

export class Code extends PageElement {
  static observedAttributes = ["lang", ...BOUND];

  constructor() {
    super();
    this.shadow.adoptedStyleSheets = [...this.shadow.adoptedStyleSheets, prose];
  }

  protected draw(): void {
    if (this.loading) {
      this.shadow.innerHTML = `<p class="faint">…</p>`;
      return;
    }
    const data = this.data;
    const wrong = failed(data);
    if (wrong) {
      this.shadow.innerHTML = `<p class="wrong">${escape(wrong)}</p>`;
      return;
    }
    const text = textOf(this, data);
    const lang = this.getAttribute("lang");
    const html = lang && hljs.getLanguage(lang) ? hljs.highlight(text, { language: lang }).value : escape(text);
    this.shadow.innerHTML = `<div class="prose"><pre><code class="hljs">${html}</code></pre></div>`;
  }
}
