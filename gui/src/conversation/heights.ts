// The heights of a conversation's blocks, in order (`view.ts`): measured
// where a block was drawn, taken to be what it holds where not. Summed in a
// Fenwick tree, so where a block begins, and which block is at a place in
// the conversation, are found in log time however many there are, and a
// block whose height changes moves only the sums above it.

export class Heights {
  private keys: readonly string[] = [];
  private at = new Map<string, number>();
  private value = new Float64Array(0);
  private tree = new Float64Array(0);
  private step = 0;

  /** Lays out `keys` afresh, each as tall as `height` says. */
  reset(keys: readonly string[], height: (key: string, index: number) => number): void {
    const n = keys.length;
    this.keys = keys;
    this.at = new Map();
    this.value = new Float64Array(n);
    this.tree = new Float64Array(n + 1);
    for (let index = 0; index < n; index += 1) {
      const key = keys[index]!;
      const value = Math.max(0, height(key, index));
      this.at.set(key, index);
      this.value[index] = value;
      // Each node adds itself to the node above it, which holds it.
      const node = index + 1;
      this.tree[node]! += value;
      const parent = node + (node & -node);
      if (parent <= n) this.tree[parent]! += this.tree[node]!;
    }
    this.step = n ? 2 ** Math.floor(Math.log2(n)) : 0;
  }

  get length(): number {
    return this.keys.length;
  }

  key(index: number): string | undefined {
    return this.keys[index];
  }

  index(key: string): number | undefined {
    return this.at.get(key);
  }

  height(index: number): number {
    return this.value[index] ?? 0;
  }

  /** A block is this tall now; says by how much it changed. */
  set(index: number, height: number): number {
    const delta = Math.max(0, height) - (this.value[index] ?? 0);
    if (!delta || index < 0 || index >= this.keys.length) return 0;
    this.value[index] = Math.max(0, height);
    for (let node = index + 1; node <= this.keys.length; node += node & -node) this.tree[node]! += delta;
    return delta;
  }

  /** How far down block `index` begins: the heights of all before it. */
  top(index: number): number {
    let sum = 0;
    for (let node = Math.min(index, this.keys.length); node > 0; node -= node & -node) sum += this.tree[node]!;
    return sum;
  }

  total(): number {
    return this.top(this.keys.length);
  }

  /** The block at `y`: the last whose top is at or above it, the first if
   *  `y` is above them all, the last if below. */
  find(y: number): number {
    const n = this.keys.length;
    if (!n) return 0;
    let index = 0;
    let rest = y;
    for (let step = this.step; step > 0; step >>= 1) {
      const next = index + step;
      if (next <= n && this.tree[next]! <= rest) {
        index = next;
        rest -= this.tree[next]!;
      }
    }
    return Math.min(index, n - 1);
  }
}
