#[cfg(windows)]
pub const WRAPPER_URL: &str = "https://world-page.invalid/host";
pub const CONTENT_URL: &str = "https://world-page.invalid/content";
pub const CHECK_URL: &str = "https://world-page-check.invalid/check";
pub const NAVIGATION: &str = include_str!("navigation.js");
/// The page views' browser has no network of its own: every request is
/// answered by the native loader, WebRTC may not use UDP outside a proxy
/// (and there is none), and no host name resolves.
#[cfg(windows)]
pub const ARGUMENTS: &str = "--enable-features=LazyBrowserInterfaceBroker --force-webrtc-ip-handling-policy=disable_non_proxied_udp --host-resolver-rules=\"MAP * ~NOTFOUND\"";
pub const POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; media-src data: blob:; font-src data: blob:; form-action 'none'; base-uri 'none'; frame-src 'none'; worker-src 'none'";
pub const WRAPPER_POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; frame-src https://world-page.invalid/content; connect-src https://world-page-check.invalid; base-uri 'none'; form-action 'none'";

/// Browser messages keep their actual WindowProxy source. The native bridge
/// exists only in this document, never in the page's JavaScript realm.
pub fn wrapper(android: bool) -> String {
    let send = if android { "worldPageNative.postMessage(JSON.stringify(value))" } else { "chrome.webview.postMessage(value)" };
    let receive = if android {
        "worldPageNative.onmessage = e => receive(JSON.parse(e.data));"
    } else {
        "chrome.webview.addEventListener('message', e => receive(e.data));"
    };
    format!(r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="x-dns-prefetch-control" content="off"><style>html,body,iframe{{margin:0;border:0;width:100%;height:100%;overflow:hidden;display:block}}</style></head><body><script>
    (async () => {{
      const send = value => {{ {send}; }};
      // Test the engine before any resident-authored bytes are parsed. The
      // native layer answers the canary locally if the header is ignored.
      try {{
        if (!window.navigation) throw new Error('this WebView does not support navigation cancellation');
        const fetchBlocked = await fetch('{CHECK_URL}').then(() => false, () => true);
        if (!fetchBlocked) throw new Error('this WebView does not enforce Connection-Allowlist; update its browser engine to open World pages');
      }} catch (error) {{
        send({{jsonrpc:'2.0',method:'world/isolation-failed',params:{{message:String(error)}}}});
        return;
      }}
      const frame = document.createElement('iframe');
      frame.sandbox = 'allow-scripts';
      frame.referrerPolicy = 'no-referrer';
      frame.title = 'World page';
      const decode = ([kind, value]) => {{
        if (kind === 0) return value;
        if (kind === 3) {{
          const bytes = atob(value);
          return Uint8Array.from(bytes, char => char.charCodeAt(0)).buffer;
        }}
        if (kind === 1) return value.map(decode);
        if (kind === 2) return Object.fromEntries(value.map(([key, item]) => [key, decode(item)]));
        throw new Error('invalid native message');
      }};
      const receive = message => frame.contentWindow.postMessage(decode(message), '*');
      {receive}
      window.addEventListener('message', e => {{
        if (e.source !== frame.contentWindow || !e.data || e.data.jsonrpc !== '2.0') return;
        send(e.data);
      }});
      // The frame loads once with the page. Any later load means the page's
      // document is gone, replaced by whatever stopped it; the host is told
      // and closes the page instead of showing an empty frame.
      let loads = 0;
      frame.addEventListener('load', () => {{
        loads += 1;
        if (loads > 1) send({{jsonrpc:'2.0',method:'world/leaving',params:{{}}}});
      }});
      frame.src = '{CONTENT_URL}';
      document.body.append(frame);
    }})();
    </script></body></html>"#)
}
