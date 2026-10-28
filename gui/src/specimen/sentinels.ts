// The sentinels' settings, as the specimen keeps them: off, or with
// `?sentinels` living on the server with a model and its key set.

import type { SentinelSeam, SentinelSettingsView } from "../world/sentinels";

export function specimenSentinels(on: boolean): SentinelSeam {
  let view: SentinelSettingsView = on
    ? {
        version: 3,
        settings: { home: "server", model: { protocol: "open_ai", endpoint: "https://api.deepseek.com/v1/chat/completions", model: "deepseek-chat" }, search: null },
        home_status: { kind: "ready" },
        keys: { model: true, search: false },
        key_error: null,
      }
    : { version: null, settings: { home: null, model: null, search: null }, home_status: { kind: "unset" }, keys: null, key_error: null };
  const wait = () => new Promise((resolve) => setTimeout(resolve, 400));
  return {
    read: async () => view,
    write: async (_hub, _account, expected, settings) => {
      await wait();
      if (expected !== view.version) return { kind: "conflict", current_version: view.version };
      view = { ...view, version: (view.version ?? 0) + 1, settings, home_status: { kind: settings.home ? "ready" : "unset" }, keys: settings.home ? (view.keys ?? { model: false, search: false }) : null };
      return { kind: "view", view };
    },
    setKey: async (_hub, _account, key) => {
      await wait();
      view = { ...view, keys: { ...(view.keys ?? { model: false, search: false }), [key]: true } };
    },
  };
}
