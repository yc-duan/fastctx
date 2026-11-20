// A collaboration stays followed briefly after its last viewer leaves.

import { effect } from "../app/signals";
import type { Store } from "../app/store";
import { names } from "../world/accounts";
import { whyOf } from "../world/again";
import { unwatch, type WorldNow } from "../world/bridge";
import { watchCollaboration, type Tree } from "../world/collab-tree";
import type { Address } from "../world/wire";

const KEEP_FOR = 60_000;
const KEPT = 5;

export interface CollaborationState {
  tree: Tree | null;
  broken: string | null;
}

type Draw = (state: CollaborationState) => void;

const pools = new WeakMap<Store, Map<string, Following>>();

export function collaborationKey(world: WorldNow | null, address: Address): string | null {
  if (!world) return null;
  const [name, hubName] = address.machine.split(".");
  if (!name) return null;
  const hubs = world.accounts.hubs.filter((hub) => {
    return (!hubName || hub.name === hubName) && hub.members.some((member) => names(member, name));
  });
  const [hub] = hubs;
  if (!hub || hubs.length !== 1 || hub.logins.length !== 1) return null;
  const member = hub.members.find((member) => names(member, name));
  const login = hub.accounts.find((account) => account.name === hub.logins[0] && !account.locked);
  const decision = world.machine_presence
    ?.find((machine) => machine.machine === address.machine)
    ?.hubs.find((one) => one.hub === hub.hub);
  if (!member || !login || !decision?.current || decision.access !== "granted") return null;
  return JSON.stringify([hub.hub, member.name, member.key, login.name, login.opened, decision.revision, address.resident]);
}

export function followCollaboration(store: Store, address: Address, draw: Draw): () => void {
  const key = collaborationKey(store.whole.peek(), address);
  let pool = pools.get(store);
  if (!pool) pools.set(store, pool = new Map());
  let followed = key === null ? undefined : pool.get(key);
  if (!followed) {
    followed = new Following(store, address, key, pool);
    if (key !== null) pool.set(key, followed);
  }
  return followed.follow(draw);
}

export function forgetCollaborations(store: Store, address?: Address): void {
  const pool = pools.get(store);
  if (!pool) return;
  for (const followed of [...pool.values()]) {
    if (!address || followed.names(address)) followed.close();
  }
}

class Following {
  private readonly viewers = new Set<Draw>();
  private state: CollaborationState = { tree: null, broken: null };
  private closed = false;
  private started = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private id: number | null = null;
  private authorization: (() => void) | null = null;

  constructor(
    private readonly store: Store,
    private readonly address: Address,
    private readonly key: string | null,
    private readonly pool: Map<string, Following>,
  ) {}

  names(address: Address): boolean {
    return address.resident === this.address.resident && address.machine === this.address.machine;
  }

  follow(draw: Draw): () => void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.viewers.add(draw);
    draw(this.state);
    if (!this.started) this.start();
    return () => {
      if (!this.viewers.delete(draw) || this.viewers.size || this.closed) return;
      const { pool, key } = this;
      if (key === null) return this.close();
      pool.delete(key);
      pool.set(key, this);
      this.timer = setTimeout(() => this.close(), KEEP_FOR);
      const idle = [...pool.values()].filter((followed) => !followed.viewers.size);
      for (const old of idle.slice(0, Math.max(0, idle.length - KEPT))) old.close();
    };
  }

  close(error: string | null = null): void {
    if (this.closed) return;
    this.closed = true;
    const { pool, key } = this;
    if (key !== null && pool.get(key) === this) pool.delete(key);
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.authorization?.();
    this.authorization = null;
    if (this.id !== null) this.unwatch(this.id);
    this.id = null;
    this.show({ tree: null, broken: error });
    this.viewers.clear();
  }

  private start(): void {
    this.started = true;
    if (this.key !== null) {
      this.authorization = effect(() => {
        if (collaborationKey(this.store.whole.get(), this.address) !== this.key) this.close();
      });
    }
    let pushed = false;
    void watchCollaboration(this.address.resident, this.address.machine, (piece) => {
      if (this.closed) return;
      if (piece.kind === "ended") {
        this.close(piece.error);
      } else if (piece.kind === "tree") {
        pushed = true;
        this.show({ tree: piece.tree, broken: null });
      } else {
        this.show({ tree: this.state.tree, broken: piece.error });
      }
    }, () => this.close(this.state.broken)).then(({ id, tree }) => {
      if (this.closed) return this.unwatch(id);
      this.id = id;
      if (!pushed) this.show({ tree, broken: this.state.broken });
    }).catch((error: unknown) => this.close(whyOf(error)));
  }

  private show(state: CollaborationState): void {
    this.state = state;
    for (const draw of [...this.viewers]) draw(state);
  }

  private unwatch(id: number): void {
    void unwatch(id).catch((error: unknown) => console.warn("a collaboration watch could not be closed", error));
  }
}
