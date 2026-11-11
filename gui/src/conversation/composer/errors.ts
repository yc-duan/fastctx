const messageOf = (error: unknown): unknown => error instanceof Error ? error.message : error;

export function tooLong(error: unknown): boolean {
  const message = messageOf(error);
  return message === "the text is longer than this input holds"
    || message === "composer: the database operation exceeds its input budget";
}

export function missingCharacter(error: unknown): boolean {
  return messageOf(error) === "composer: the character is not in this draft";
}

export function recoverable(error: unknown): boolean {
  const message = messageOf(error);
  return tooLong(error) || missingCharacter(error)
    || message === "that shared document no longer exists"
    || message === "that shared document is being delivered";
}
