/** Yjs bytes use base64 at the JSON boundary. */
export const encode = (bytes: Uint8Array): string => {
  let value = "";
  for (let at = 0; at < bytes.length; at += 8192) value += String.fromCharCode(...bytes.subarray(at, at + 8192));
  return btoa(value);
};
export const decode = (value: string): Uint8Array => Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
