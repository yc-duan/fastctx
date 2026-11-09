// Features not offered in a release yet (`world.md`, 这一版不发的): sentinels
// and goals. Their code stays and a development build of the node runs them;
// only the development server shows them, so no release shows the user a
// way in.

export const PREVIEW: boolean = import.meta.env.DEV;

/** Whether releases offer history, as the node's `HISTORY_SHIPS` says
 *  (`crates/fastctx-platform/src/home.rs`). */
const HISTORY_SHIPS = false;

/** History has a switch of its own: shown wherever the preview is, and in a
 *  release once it ships there. It is a part of the workbench, so it
 *  appears only with it. */
export const HISTORY: boolean = HISTORY_SHIPS || PREVIEW;
