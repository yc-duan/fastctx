/** Consecutive failures wait 1, 2, 4, ... seconds, up to half a minute. */
export class Backoff {
  private delay = 0;

  failed(): number {
    this.delay = Math.min(this.delay ? this.delay * 2 : 1_000, 30_000);
    return this.delay;
  }

  reset(): void {
    this.delay = 0;
  }
}
