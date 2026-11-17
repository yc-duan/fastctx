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
  failed: "Fastctx 未能打开",
  steps: {
    open_connection: "无法连接本设备上的 Fastctx。",
    interface: "界面未能载入，请重试。",
  },
  stepOther: "打开时出错。",
  retry: "重试",
  reopening: "已修复，正在重新打开",
  repair: zhRepair,
};

export const enStartup: StartupWords = {
  opening: "Opening Fastctx",
  failed: "Fastctx did not open",
  steps: {
    open_connection: "Could not connect to Fastctx on this device.",
    interface: "The interface could not load. Try again.",
  },
  stepOther: "Something went wrong while opening.",
  retry: "Try again",
  reopening: "Repaired, opening again",
  repair: enRepair,
};
