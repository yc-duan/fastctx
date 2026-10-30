// Features not offered in a release yet (`world.md`, 这一版不发的): sentinels,
// project memory and goals. Their code stays and a development build of the
// node runs them; only the development server shows them, so no release
// shows the user a way in.

export const PREVIEW: boolean = import.meta.env.DEV;
