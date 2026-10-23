// The first thing the page runs: whether the browser engine it is drawn
// by can draw the interface at all, before any of it loads. The newest
// thing the interface's styles use is `color-mix()`, which Chromium has
// from 111 (WebView2 on Windows, the system WebView on Android); `:has`,
// container queries, `dvh`, `lh` and the highlight API all came before it.
// An older engine would draw a page out of joint, or nothing; it is told
// so plainly instead, with what to update.

const MINIMUM = 111;

function canDraw(): boolean {
  return (
    typeof CSS !== "undefined" &&
    CSS.supports("color", "color-mix(in srgb, red 50%, blue)") &&
    CSS.supports("selector(:has(a))") &&
    CSS.supports("container-type: inline-size") &&
    CSS.supports("height: 1dvh") &&
    CSS.supports("height: 1lh") &&
    "highlights" in CSS
  );
}

function tooOld(): void {
  const found = /Chrom(?:e|ium)\/(\d+)/.exec(navigator.userAgent)?.[1] ?? null;
  const android = /Android/i.test(navigator.userAgent);
  const zh = !/^en/i.test(new URLSearchParams(location.search).get("lang") ?? navigator.language);
  const light = document.documentElement.dataset.theme === "warm-white";
  // A new enough engine can still lack what is used, where a policy turns
  // it off: said as that, not as an old version.
  const lacking = found !== null && Number(found) >= MINIMUM;
  const title = lacking
    ? zh
      ? "这台设备的浏览器内核画不了这个界面"
      : "This device's browser engine cannot draw this interface"
    : zh
      ? "这台设备的浏览器内核太旧了"
      : "This device's browser engine is too old";
  const why = lacking
    ? zh
      ? `这台设备的系统浏览器内核（Chromium ${found}）缺少界面要用的绘制能力。`
      : `This device's browser engine (Chromium ${found}) lacks what the interface is drawn with.`
    : zh
      ? `Fastctx 的界面要用 Chromium ${MINIMUM} 或更新的系统浏览器内核来画，这台设备上的是 ${found ? `Chromium ${found}` : "更早的版本"}。`
      : `Fastctx's interface is drawn by the system's browser engine, which must be Chromium ${MINIMUM} or newer; this device has ${found ? `Chromium ${found}` : "an older one"}.`;
  const what = android
    ? zh
      ? "在应用商店里更新「Android System WebView」（有的手机上叫「Chrome」），然后重新打开这个应用。"
      : "Update “Android System WebView” (on some phones, “Chrome”) from the app store, then open this app again."
    : zh
      ? "更新 Microsoft Edge WebView2 运行时，然后重新打开这个应用。"
      : "Update the Microsoft Edge WebView2 Runtime, then open this app again.";
  const page = document.createElement("main");
  page.setAttribute("style", `min-height:100vh;display:flex;flex-direction:column;justify-content:center;padding:32px 24px;box-sizing:border-box;max-width:560px;margin:0 auto;font:16px/1.7 system-ui,sans-serif;color:${light ? "#0d0d0d" : "#f8f8f6"}`);
  const heading = document.createElement("h1");
  heading.setAttribute("style", "font-size:20px;font-family:Emphasis,Reading,serif;font-weight:400;margin:0 0 12px");
  heading.textContent = title;
  page.append(heading);
  for (const line of [why, what]) {
    const paragraph = document.createElement("p");
    paragraph.setAttribute("style", "margin:0 0 8px");
    paragraph.textContent = line;
    page.append(paragraph);
  }
  document.body.setAttribute("style", `margin:0;background:${light ? "#f8f8f6" : "#1f1f1e"}`);
  document.getElementById("app")?.replaceWith(page);
}

// Inside the shell the World on this device opens after the window does:
// the interface loads once it has (`startup/gate.ts`). The language is
// settled next: the interface reads it as it starts.
if (canDraw())
  void import("./startup/gate")
    .then(({ untilStarted }) => untilStarted())
    .then(() => import("./language"))
    .then(({ settleLanguage }) => settleLanguage())
    .then(() => import("./main"));
else tooOld();
