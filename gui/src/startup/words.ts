// The words of the page shown while Fastctx opens and when it could not.

import { enRepair, zhRepair, type RepairWords } from "../app/repair-words";

export interface StartupWords {
  opening: string;
  failed: string;
  /** What did not open, by the step that said so. */
  steps: Record<string, string>;
  stepOther: string;
  retry: string;
  /** Repaired: the opening is tried again by itself. */
  reopening: string;
  repair: RepairWords;
}

export const zhStartup: StartupWords = {
  opening: "正在打开 Fastctx",
  failed: "Fastctx 没能打开",
  steps: {
    open_connection: "没连上这台设备上的 Fastctx。",
  },
  stepOther: "打开时出了问题。",
  retry: "重试",
  reopening: "修好了，正在重新打开",
  repair: zhRepair,
};

export const enStartup: StartupWords = {
  opening: "Opening Fastctx",
  failed: "Fastctx didn't open",
  steps: {
    open_connection: "Couldn't connect to Fastctx on this device.",
  },
  stepOther: "Something went wrong while opening.",
  retry: "Try again",
  reopening: "Repaired, opening again",
  repair: enRepair,
};
