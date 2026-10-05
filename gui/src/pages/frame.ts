// Where a page is put: a frame in the interface's document that runs the
// page's script and nothing more. Without `allow-same-origin` its origin
// is opaque, so it reaches none of the interface's storage, commands or
// windows; without the other permissions it cannot navigate anything but
// itself, open windows, submit forms, show dialogs or download. Its own
// policy (`compose.ts`) keeps it off the network. What it may do it asks
// the host for, through the bridge.
//
// How a frame is kept closed differs by platform; this is the browser's own
// sandbox, which every WebView the interface runs in honours.

export function mountFrame(into: HTMLElement, html: string, title: string): HTMLIFrameElement {
  const frame = document.createElement("iframe");
  frame.className = "page-frame";
  frame.title = title;
  frame.setAttribute("sandbox", "allow-scripts");
  frame.setAttribute("referrerpolicy", "no-referrer");
  frame.srcdoc = html;
  into.append(frame);
  return frame;
}
