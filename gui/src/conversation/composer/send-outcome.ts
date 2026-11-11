export const ALREADY_SENT = "these words were already sent";
export const SEND_WITHDRAWN = "this send was withdrawn";
export type Withdrawal = { readonly kind: "withdrawn" | "sent" } | { readonly kind: "unavailable"; readonly why: string };

/** A confirmed cancellation also releases later locally sealed sends. */
export class Withdrawn extends Error {
  constructor() { super("words sent before these did not go"); }
}
