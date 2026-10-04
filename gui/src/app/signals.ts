// A very small reactive core: values that say when they change, and
// effects that run again when a value they read changes. The interface
// keeps the World in a few of these, and every part that shows the World
// reads them, so the sidebar and a page never disagree.

interface Reaction {
  run(): void;
  sources: Set<Signal<unknown>>;
  stopped: boolean;
}

let running: Reaction | null = null;
let holding = 0;
const due = new Set<Reaction>();

export class Signal<T> {
  private readonly readers = new Set<Reaction>();

  constructor(
    private value: T,
    private readonly same: (a: T, b: T) => boolean = Object.is,
  ) {}

  /** The value, noting that whatever runs now depends on it. */
  get(): T {
    if (running && !running.stopped) {
      this.readers.add(running);
      running.sources.add(this as Signal<unknown>);
    }
    return this.value;
  }

  /** The value, without depending on it. */
  peek(): T {
    return this.value;
  }

  set(value: T): void {
    if (this.same(this.value, value)) return;
    this.value = value;
    for (const reader of [...this.readers]) due.add(reader);
    if (holding === 0) flush();
  }

  update(change: (value: T) => T): void {
    this.set(change(this.value));
  }

  forget(reader: Reaction): void {
    this.readers.delete(reader);
  }
}

export function signal<T>(value: T, same?: (a: T, b: T) => boolean): Signal<T> {
  return new Signal(value, same);
}

function flush(): void {
  while (due.size) {
    const [next] = due;
    due.delete(next!);
    if (!next!.stopped) next!.run();
  }
}

/** Runs `body` now and again whenever a value it read changes; what it
 *  returns is called before each next run and when it is stopped. Returns
 *  what stops it. */
export function effect(body: () => void | (() => void)): () => void {
  let cleanup: void | (() => void);
  const reaction: Reaction = {
    sources: new Set(),
    stopped: false,
    run() {
      for (const source of reaction.sources) source.forget(reaction);
      reaction.sources.clear();
      if (typeof cleanup === "function") cleanup();
      const outer = running;
      running = reaction;
      try {
        cleanup = body();
      } finally {
        running = outer;
      }
    },
  };
  reaction.run();
  return () => {
    reaction.stopped = true;
    for (const source of reaction.sources) source.forget(reaction);
    reaction.sources.clear();
    if (typeof cleanup === "function") cleanup();
  };
}

/** Changes several values and lets what depends on them run once after. */
export function batch(change: () => void): void {
  holding += 1;
  try {
    change();
  } finally {
    holding -= 1;
    if (holding === 0) flush();
  }
}
