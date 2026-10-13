// A project's "⋯" (`world-gui.md`, 账号、机器、共享与协助): its owner shares
// it with other accounts on its hub and takes it back, and hands it to
// another account; the network's administrator hands any project on. A
// project shared with the reader says whose it is.

import { escape } from "../conversation/prose";
import { loginsOf, type AccountChange, type HubAccounts, type Login, type ProjectSeen } from "../world/accounts";
import type { AccessWords } from "./account-words";
import { ActsMenu, type MenuAct } from "./acts-menu";
import type { Confirm } from "./confirm";
import { GiveSheet } from "./machine-sheets";
import { isSuper } from "./reach";
import type { Store } from "./store";

export interface ProjectActsWords {
  more: string;
  share: string;
  transfer: string;
  shareTitle(project: string): string;
  shareWhat: string;
  shareSees: string;
  sharedTo: string;
  shareGo: string;
  transferTitle(project: string): string;
  transferWhat: string;
  transferTo: string;
  transferGo: string;
  sharedBy(owner: string): string;
}

export const zhProjectActs: ProjectActsWords = {
  more: "更多",
  share: "共享…",
  transfer: "转交…",
  shareTitle: (project) => `共享「${project}」`,
  shareWhat: "对方能读、能写这个项目的记录，也能在里面开对话。",
  shareSees: "对方在项目里写下的，你都看得到",
  sharedTo: "已共享给",
  shareGo: "共享",
  transferTitle: (project) => `转交「${project}」`,
  transferWhat: "项目和它的记录从此归对方所有。",
  transferTo: "交给",
  transferGo: "转交",
  sharedBy: (owner) => `${owner} 共享给你`,
};

export const enProjectActs: ProjectActsWords = {
  more: "More",
  share: "Share…",
  transfer: "Hand over…",
  shareTitle: (project) => `Share “${project}”`,
  shareWhat: "They can read and write the project's records, and start conversations in it.",
  shareSees: "You see everything they write in the project",
  sharedTo: "Shared with",
  shareGo: "Share",
  transferTitle: (project) => `Hand over “${project}”`,
  transferWhat: "The project and its records will belong to them from now on.",
  transferTo: "To",
  transferGo: "Hand over",
  sharedBy: (owner) => `shared with you by ${owner}`,
};

export interface ProjectActsOptions {
  words: ProjectActsWords;
  access: AccessWords;
  store: Store;
  confirm: Confirm;
  change(login: Login, change: AccountChange): Promise<void>;
}

/** How the reader holds a project: as which login, on which hub, whether
 *  it is theirs, and whether they are the network's administrator. */
interface Hold {
  login: Login;
  hub: HubAccounts;
  project: ProjectSeen;
  mine: boolean;
  admin: boolean;
}

export class ProjectActs {
  private readonly menu = new ActsMenu();
  private readonly giving: GiveSheet;

  constructor(private readonly options: ProjectActsOptions) {
    this.giving = new GiveSheet(options.access);
  }

  /** What the reader may do to `town` from its "⋯". */
  acts(town: string): MenuAct[] {
    const hold = this.hold(town);
    const { words } = this.options;
    if (!hold) return [];
    return [
      ...(hold.mine ? [{ act: "share", icon: "colleague" as const, label: words.share }] : []),
      ...(hold.mine || hold.admin ? [{ act: "transfer", icon: "move" as const, label: words.transfer }] : []),
    ];
  }

  /** Whose `town` is, where it is another's shared with the reader. */
  sharedBy(town: string): string | null {
    const hold = this.hold(town);
    return hold && !hold.mine && hold.project.granted.includes(hold.login.account) ? hold.project.owner : null;
  }

  open(town: string, name: string, anchor: HTMLElement): void {
    this.menu.open(anchor, name, this.acts(town), (act) => {
      if (act === "share") this.share(town, name);
      else if (act === "transfer") this.transfer(town, name);
    });
  }

  private share(town: string, name: string): void {
    const hold = this.hold(town);
    if (!hold) return;
    const { words, change, store } = this.options;
    this.giving.open(
      { title: words.shareTitle(name), what: words.shareWhat, sees: words.shareSees, given: words.sharedTo, go: words.shareGo },
      hold.project.granted,
      async (to, shared) => {
        await change(hold.login, { change: "share_project", town, with: to, shared });
        await store.refresh();
        return this.hold(town)?.project.granted ?? [];
      },
    );
  }

  private transfer(town: string, name: string): void {
    const hold = this.hold(town);
    if (!hold) return;
    const { words, change, store, confirm } = this.options;
    const others = hold.hub.accounts.filter((account) => account.name !== hold.project.owner && !account.locked);
    if (!others.length) return;
    confirm.ask({
      title: words.transferTitle(name),
      what: words.transferWhat,
      fields: `<label class="field"><span class="field-label">${escape(words.transferTo)}</span><select name="to" required>${others
        .map((account) => `<option value="${escape(account.name)}">${escape(account.name)}</option>`)
        .join("")}</select></label>`,
      go: words.transferGo,
      act: async () => {
        const to = document.querySelector<HTMLSelectElement>("dialog[open].confirm-sheet select[name=to]")?.value;
        if (to) await change(hold.login, { change: "move_project", town, to });
      },
      done: () => void store.refresh(),
    });
  }

  private hold(town: string): Hold | null {
    const world = this.options.store.world.peek();
    if (!world) return null;
    for (const hub of world.accounts.hubs) {
      const project = hub.projects.find((one) => one.name === town);
      if (!project) continue;
      const logins = loginsOf(world.accounts).filter((login) => login.hub === hub.hub);
      const owner = logins.find((login) => login.account === project.owner);
      const login = owner ?? logins[0];
      if (!login) continue;
      return { login, hub, project, mine: !!owner, admin: logins.some((one) => isSuper(world, one)) };
    }
    return null;
  }
}
