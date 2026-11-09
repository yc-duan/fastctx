// Development-only specimen data; account opt-ins govern shipped features.

export const PREVIEW: boolean = import.meta.env.DEV;

/** Whether releases offer history, as the node's `HISTORY_SHIPS` says
 *  (`crates/fastctx-platform/src/home.rs`). */
const HISTORY_SHIPS = true;

/** The account opt-in controls notes within the workbench. */
export const HISTORY: boolean = HISTORY_SHIPS;
