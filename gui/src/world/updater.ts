// The app's own updates, from the project's GitHub releases, through Tauri's
// official updater: the signed installer is checked against the key built
// into the app (`plugins.updater` in `src-tauri/tauri.conf.json`). Only used
// on a computer; a phone updates another way.

import { getVersion } from "@tauri-apps/api/app";
import { check } from "@tauri-apps/plugin-updater";
import type { Available, Updating } from "../app/app-update";

export const updating: Updating = {
  current: () => getVersion(),
  async check(): Promise<Available | null> {
    const update = await check();
    if (!update) return null;
    const available: Available = {
      version: update.version,
      notes: update.body ?? null,
      async install(progress) {
        let received = 0;
        let total: number | null = null;
        // On Windows the installer takes over and the app exits here, so
        // this promise does not resolve; the row shows "installing" from
        // the download finishing. Elsewhere it resolves and the app relaunches.
        await update.downloadAndInstall((event) => {
          if (event.event === "Started") total = event.data.contentLength ?? null;
          else if (event.event === "Progress") {
            received += event.data.chunkLength;
            progress(received, total);
          } else if (event.event === "Finished") progress(total ?? received, total ?? received);
        });
      },
    };
    return available;
  },
};
