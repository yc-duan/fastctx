// Shared JSON representations of fastctx-types names and serde primitives.
// Name validation belongs to Rust; these newtypes serialize as plain strings.
export type MachineName = string;
export type ResidentId = string;
export type NoticeId = string;
export type TownName = string;
export type JournalName = string;
export type EntryId = string;
export type AgentName = string;
export type JobId = string;

/** jiff::Timestamp emits a UTC ISO 8601 string with up to nine fractional digits. */
export type Timestamp = string;

/** serde_json::Value, including explicit null but never undefined. */
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

// Rust integers are JSON numbers. JSON.parse rounds integers outside JS's
// safe range; preserving those values requires a lossless transport decoder.

/** serde's externally tagged Result, preserving the capitalized variant names. */
export type Result<T, E> = { Ok: T; Err?: never } | { Err: E; Ok?: never };

export interface Address {
  machine: MachineName;
  resident: ResidentId;
}
