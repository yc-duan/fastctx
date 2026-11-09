import { invoke } from "@tauri-apps/api/core";
import { StartupPage } from "./page";
import { enStartup, zhStartup } from "./words";

/** A boot failure must remain readable even when a later module cannot load. */
export function showStartupFailure(error: unknown): void {
  const detail = (error instanceof Error ? error.stack ?? error.message : String(error)).slice(0, 16000);
  console.error("Fastctx interface startup failed", error);
  const asked = new URLSearchParams(location.search).get("lang");
  let language = asked;
  try { language ??= localStorage.getItem("lang"); } catch { /* The page can report a storage failure without using storage. */ }
  const words = language === "en" || (language !== "zh" && !/^zh/i.test(navigator.language)) ? enStartup : zhStartup;
  const page = new StartupPage(words, { retry: () => location.reload(), repair: null });
  page.failed({ step: "interface", error: detail });
  document.querySelectorAll(".su-page").forEach((old) => old.remove());
  document.body.append(page.element);
  if ("__TAURI_INTERNALS__" in window) {
    void invoke("startup_frontend_failed", { error: detail }).catch((failure: unknown) => console.error("Recording interface startup failure", failure));
  }
}
