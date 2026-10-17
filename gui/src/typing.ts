// Keys pressed while an input method composes (Chinese, Japanese, Korean):
// the Enter that picks a candidate is not the Enter that sends.

/** Whether `event` belongs to an input method's composition: its own flag,
 *  or the key code 229 some systems give the key that ends one. */
export function composing(event: KeyboardEvent): boolean {
  return event.isComposing || event.keyCode === 229;
}
