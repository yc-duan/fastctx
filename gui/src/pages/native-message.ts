/** Preserve ArrayBuffer values across the native JSON transport without
 * reserving any property names in the page's JSON-RPC payload. */
export function nativeMessage(value: unknown): unknown {
  if (value instanceof ArrayBuffer) {
    let binary = "";
    for (const byte of new Uint8Array(value)) binary += String.fromCharCode(byte);
    return [3, btoa(binary)];
  }
  if (Array.isArray(value)) return [1, value.map(nativeMessage)];
  if (value !== null && typeof value === "object") {
    return [2, Object.entries(value).map(([key, item]) => [key, nativeMessage(item)])];
  }
  return [0, value];
}
