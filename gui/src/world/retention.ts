// The rule a machine deletes its conversations idle long enough by
// (`fastctx_types::conversation_retention`), as the World answers it.

export const RETENTION_DAYS = [3, 7, 14, 30, 60, 90] as const;

export type RetentionAsk =
  | { do: "read" }
  | { do: "owner"; own_days: number | null; assisted_days: number | null }
  | { do: "mine"; days: number | null };

export interface RetentionRule {
  own_days?: number;
  assisted_days?: number;
  accounts?: { of: { hub: string; account: string }; days: number }[];
  grace_until?: string;
}

export interface RetentionDue {
  resident: string;
  idle_since: string;
  days?: number;
  at?: string;
}

export interface RetentionAnswer {
  rule: RetentionRule;
  /** Whether the asker is the machine's owner. */
  owner: boolean;
  conversations: RetentionDue[];
}
