// Moving through a menu's items by keys: the arrows step round, Home and End
// go to the first and the last.

/** The index `key` moves to among `items` from the one focused, or `null`
 *  where the key moves nothing. */
export function stepped(items: readonly HTMLElement[], focused: Element | null, key: string): number | null {
  if (!items.length) return null;
  const at = items.indexOf(focused as HTMLElement);
  switch (key) {
    case "ArrowDown":
      return (at + 1) % items.length;
    case "ArrowUp":
      return at < 0 ? items.length - 1 : (at - 1 + items.length) % items.length;
    case "Home":
      return 0;
    case "End":
      return items.length - 1;
    default:
      return null;
  }
}
