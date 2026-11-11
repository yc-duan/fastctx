/** A logical clock: timers run only when the schedule explicitly advances. */
export class Clock {
  now = 1_000;
  serial = 0;
  tasks = new Map();
  install(seed = 1) {
    const originalDate = globalThis.Date;
    const clock = this;
    globalThis.Date = class extends originalDate {
      constructor(...args) { super(...(args.length ? args : [clock.now])); }
      static now() { return clock.now; }
    };
    globalThis.setTimeout = (run, after = 0, ...args) => {
      const id = ++this.serial;
      this.tasks.set(id, { at: this.now + after, run: () => run(...args) });
      return id;
    };
    globalThis.clearTimeout = (id) => this.tasks.delete(id);
    let state = seed >>> 0 || 1;
    const random = () => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return state >>> 0; };
    Math.random = () => random() / 0x1_0000_0000;
    crypto.getRandomValues = (array) => { for (let at = 0; at < array.length; at++) array[at] = random(); return array; };
    crypto.randomUUID = () => `00000000-0000-4000-8000-${random().toString(16).padStart(12, "0")}`;
  }
  async flush() { for (let turn = 0; turn < 32; turn++) await Promise.resolve(); }
  async advance(after) {
    const until = this.now + after;
    for (;;) {
      const next = [...this.tasks].filter(([, task]) => task.at <= until).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!next) break;
      this.now = next[1].at; this.tasks.delete(next[0]); next[1].run(); await this.flush();
    }
    this.now = until; await this.flush();
  }
}
