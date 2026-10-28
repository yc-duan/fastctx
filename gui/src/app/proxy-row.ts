// This computer's proxy to its hubs, a row of the settings' device section
// (`world-network.md`, 经代理连中枢): what it is, changed in place in the
// field renaming uses; emptied, the system's settings again.

import { escape } from "../conversation/prose";
import "./proxy-row.css";

export interface ProxyRowWords {
  proxy: string;
  proxyShort: string;
  proxyWhat: string;
  proxySystem: string;
  proxyChange: string;
  proxySave: string;
  proxyCancel: string;
  proxyWrong: string;
  proxyFailed: string;
}

export interface ProxySeam {
  get(): Promise<string | null>;
  /** Sets it, or clears it with none; the proxy as it is now. */
  set(url: string | null): Promise<string | null>;
}

/** `url` as it is shown: a password in it starred, as the node logs it. */
function shown(url: string): string {
  return url.replace(/^((?:[a-z][a-z0-9+.-]*:\/\/)?[^/@:]*:)[^/@]*@/i, "$1***@");
}

export class ProxyRow {
  readonly element: HTMLElement;
  private url: string | null = null;
  private editing = false;
  private busy = false;
  private wrong: { said: string; why: string } | null = null;
  /** What is in the field while it is changed, kept when a save fails. */
  private typed = "";

  constructor(
    private readonly words: ProxyRowWords,
    private readonly seam: ProxySeam,
  ) {
    this.element = document.createElement("dl");
    this.element.className = "facts proxy-row";
    this.element.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLElement>("[data-proxy]")?.dataset.proxy;
      if (act === "change") this.edit();
      else if (act === "cancel") this.stop();
      else if (act === "save") void this.save();
    });
    this.element.addEventListener("keydown", (event) => {
      if (!this.editing) return;
      if (event.key === "Enter") {
        event.preventDefault();
        void this.save();
      } else if (event.key === "Escape") {
        event.preventDefault();
        this.stop();
      }
    });
    this.draw();
    seam.get().then(
      (url) => {
        this.url = url;
        this.draw();
      },
      () => this.draw(),
    );
  }

  private draw(): void {
    const { words } = this;
    const label = `<dt title="${escape(words.proxyWhat)}">${escape(words.proxyShort)}</dt>`;
    if (!this.editing) {
      this.element.innerHTML = `${label}<dd class="proxy-row-value"><span${this.url ? ' class="mono"' : ""}>${escape(this.url ? shown(this.url) : words.proxySystem)}</span></dd><dd class="sd-act"><button class="text-button quiet-button" type="button" data-proxy="change">${escape(words.proxyChange)}</button></dd>`;
      return;
    }
    const said = this.wrong ? `<p class="rename-said wrong" title="${escape(this.wrong.why)}">${escape(this.wrong.said)}</p>` : "";
    this.element.innerHTML = `${label}<dd class="sd-wide"><div class="rename-field${this.busy ? " busy" : ""}"><div class="rename-row"><input class="rename-input mono" name="proxy" spellcheck="false" autocapitalize="off" autocomplete="off" placeholder="http://host:port" aria-label="${escape(words.proxy)}" value="${escape(this.typed)}"${this.busy ? " readonly" : ""}><button class="text-button" type="button" data-proxy="save">${escape(words.proxySave)}</button><button class="text-button quiet-button" type="button" data-proxy="cancel">${escape(words.proxyCancel)}</button></div>${said}</div></dd>`;
  }

  private edit(): void {
    this.editing = true;
    this.wrong = null;
    this.typed = this.url ?? "";
    this.draw();
    const input = this.element.querySelector<HTMLInputElement>("input[name=proxy]");
    input?.focus();
    input?.select();
  }

  private stop(): void {
    if (this.busy) return;
    this.editing = false;
    this.wrong = null;
    this.draw();
  }

  private async save(): Promise<void> {
    if (this.busy) return;
    this.typed = this.element.querySelector<HTMLInputElement>("input[name=proxy]")?.value ?? this.typed;
    this.busy = true;
    this.draw();
    try {
      this.url = await this.seam.set(this.typed.trim() || null);
      this.editing = false;
      this.wrong = null;
    } catch (error) {
      const why = error instanceof Error ? error.message : String(error);
      this.wrong = { said: why.includes("no HTTP proxy") ? this.words.proxyWrong : this.words.proxyFailed, why };
    } finally {
      this.busy = false;
      this.draw();
    }
  }
}
