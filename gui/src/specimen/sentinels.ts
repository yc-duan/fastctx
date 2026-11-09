// The sentinels' settings, as the specimen keeps them: off, or with
// `?sentinels` living on the server with a model and its key set. A try
// finds the model answering; with `?unkeyed` its key refused.

import type { SentinelSeam, SentinelSettingsView } from "../world/sentinels";

export function specimenSentinels(on: boolean): SentinelSeam {
  let view: SentinelSettingsView = on
    ? {
        version: 3,
        settings: { sentinels: true, goals: true, home: "server", model: { protocol: "open_ai", endpoint: "https://api.deepseek.com", model: "deepseek-chat" }, search: null },
        home_status: { kind: "ready" },
        keys: { model: true, search: false },
        key_error: null,
      }
    : { version: null, settings: { home: null, model: null, search: null }, home_status: { kind: "unset" }, keys: null, key_error: null };
  const initial = structuredClone(view);
  const accounts = new Map<string, SentinelSettingsView>();
  const get = (hub: string, account: string) => accounts.get(JSON.stringify([hub, account])) ?? structuredClone(initial);
  const keep = (hub: string, account: string) => accounts.set(JSON.stringify([hub, account]), view);
  const wait = (ms = 400) => new Promise((resolve) => setTimeout(resolve, ms));
  const refused = new URLSearchParams(location.search).has("unkeyed");
  return {
    read: async (hub, account) => get(hub, account),
    write: async (_hub, _account, expected, settings) => {
      await wait();
      view = get(_hub, _account);
      if (expected !== view.version) return { kind: "conflict", current_version: view.version };
      view = { ...view, version: (view.version ?? 0) + 1, settings, home_status: { kind: settings.home ? "ready" : "unset" }, keys: settings.home ? (view.keys ?? { model: false, search: false }) : null };
      keep(_hub, _account);
      return { kind: "view", view };
    },
    setKey: async (_hub, _account, key) => {
      await wait();
      view = get(_hub, _account);
      view = { ...view, keys: { ...(view.keys ?? { model: false, search: false }), [key]: true } };
      keep(_hub, _account);
    },
    tryOut: async (hub, account) => {
      await wait(1200);
      const view = get(hub, account);
      const model = view.settings.model
        ? refused
          ? { ok: false, ms: 412, said: 'https://api.deepseek.com/chat/completions answered HTTP 401 (authentication_error/invalid_api_key): {"error":{"message":"Authentication Fails, Your api key: ****abcd is invalid","type":"authentication_error"}}' }
          : { ok: true, ms: 1384, said: "Ready." }
        : null;
      const search = view.settings.search ? { ok: true, ms: 822, said: "5 results" } : null;
      return { model, search };
    },
  };
}
