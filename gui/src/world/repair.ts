// A repair of a machine's Fastctx, as its record says (`fastctx_types::repair`):
// the layers it went through, each checked and left as it was, mended, or
// not mended, and how it ended.

export type LayerOutcome = "checking" | "unchanged" | "repaired" | "failed";

export interface RepairStep {
  layer: string;
  outcome: LayerOutcome;
  detail: string;
  actions: string[];
}

export interface RepairReport {
  id: string;
  version: string;
  by?: string;
  started?: string;
  finished: string | null;
  steps: RepairStep[];
  success: boolean;
  unchanged: boolean;
  error: string | null;
}
