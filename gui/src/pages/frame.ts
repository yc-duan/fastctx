import { Channel, invoke } from "@tauri-apps/api/core";
import { nativeMessage } from "./native-message";
import { inShell } from "../world/bridge";

/** A page's transport and lifetime. Native surfaces have a real parent
 * document of their own, outside the interface's Tauri webview. */
export interface PageFrame {
  listen(message: (value: unknown) => void, failed: (error?: string) => void): void;
  post(message: object): void;
  close(): void;
}

interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  visible: boolean;
}

type Delivery = { kind: "message"; value: unknown } | { kind: "failed"; value: string };

/** Browser previews retain the browser sandbox. Packaged applications use
 * the platform module; a mount failure never falls back to a Tauri iframe. */
export function mountFrame(into: HTMLElement, html: string, title: string): PageFrame {
  return inShell() ? nativeFrame(into, html, title) : browserFrame(into, html, title);
}

function browserFrame(into: HTMLElement, html: string, title: string): PageFrame {
  const frame = document.createElement("iframe");
  frame.className = "page-frame";
  frame.title = title;
  frame.setAttribute("sandbox", "allow-scripts");
  frame.setAttribute("referrerpolicy", "no-referrer");
  frame.srcdoc = html;
  into.append(frame);
  let receive: (value: unknown) => void = () => {};
  let failed: () => void = () => {};
  let loads = 0;
  const heard = (event: MessageEvent) => {
    if (event.source && event.source === frame.contentWindow) receive(event.data);
  };
  const loaded = () => { if (++loads > 1) failed(); };
  window.addEventListener("message", heard);
  frame.addEventListener("load", loaded);
  return {
    listen(message, left) { receive = message; failed = left; },
    post(message) { frame.contentWindow?.postMessage(message, "*"); },
    close() {
      window.removeEventListener("message", heard);
      frame.removeEventListener("load", loaded);
      frame.remove();
    },
  };
}

function nativeFrame(into: HTMLElement, html: string, title: string): PageFrame {
  const element = document.createElement("div");
  element.className = "page-frame";
  element.setAttribute("aria-label", title);
  into.append(element);
  const id = crypto.randomUUID();
  let closed = false;
  let receive: (value: unknown) => void = () => {};
  let failed: (error?: string) => void = () => {};
  const report = (error: unknown) => { if (!closed) failed(String(error)); };
  const messages = new Channel<Delivery>();
  messages.onmessage = delivery => {
    if (closed) return;
    if (delivery.kind === "message") receive(delivery.value);
    else report(delivery.value);
  };
  const bounds = (): Bounds => {
    const rect = element.getBoundingClientRect();
    const visible = element.isConnected && rect.width > 0 && rect.height > 0
      && !document.querySelector("dialog[open]")
      && [[rect.left + 1, rect.top + 1], [rect.right - 1, rect.top + 1], [rect.left + 1, rect.bottom - 1], [rect.right - 1, rect.bottom - 1], [rect.left + rect.width / 2, rect.top + rect.height / 2]]
        .every(([x, y]) => { const top = document.elementFromPoint(x!, y!); return top === element || (top !== null && element.contains(top)); });
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height, scale: devicePixelRatio, visible };
  };
  const mounted = invoke("plugin:world-pages|mount", { id, html, bounds: { ...bounds(), visible: false }, messages });
  let chain: Promise<unknown> = mounted;
  let last = "";
  let scheduled = 0;
  const place = () => {
    scheduled = 0;
    if (closed) return;
    const value = bounds();
    const current = JSON.stringify(value);
    if (current === last) return;
    last = current;
    chain = chain.then(() => closed ? undefined : invoke("plugin:world-pages|place", { id, bounds: value }));
    void chain.catch(report);
  };
  const schedule = () => { if (!scheduled && !closed) scheduled = requestAnimationFrame(place); };
  const resize = new ResizeObserver(schedule);
  resize.observe(element);
  const mutation = new MutationObserver(schedule);
  mutation.observe(document.body, { attributes: true, childList: true, subtree: true });
  window.addEventListener("resize", schedule);
  window.addEventListener("scroll", schedule, true);
  document.addEventListener("transitionend", schedule, true);
  void mounted.then(schedule, report);
  return {
    listen(message, left) { receive = message; failed = left; },
    post(message) {
      if (closed) return;
      const serializable = nativeMessage(message);
      chain = chain.then(() => closed ? undefined : invoke("plugin:world-pages|post", { id, message: serializable }));
      void chain.catch(report);
    },
    close() {
      if (closed) return;
      closed = true;
      cancelAnimationFrame(scheduled);
      resize.disconnect();
      mutation.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      document.removeEventListener("transitionend", schedule, true);
      element.remove();
      // A close racing creation still destroys the newly created view.
      void mounted.then(() => invoke("plugin:world-pages|close", { id })).catch(error => console.error("closing native page", error));
    },
  };
}
