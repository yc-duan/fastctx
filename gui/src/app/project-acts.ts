// A project's "⋯" (`world-gui.md`, 账号、机器、共享与协助): its owner shares
// it with other accounts on its hub and takes it back, hands it to another
// account, and deletes it with the conversations in it. A project shared
// with the reader says whose it is.

import { escape } from "../conversation/prose";
import { loginsOf, type AccountChange, type HubAccounts, type Login, type ProjectSeen } from "../world/accounts";
import type { ResidentNow } from "../world/world";
import type { AccessWords } from "./account-words";
import { ActsMenu, type MenuAct } from "./acts-menu";
import type { Confirm } from "./confirm";
import { GiveSheet } from "./machine-sheets";
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
  remove: string;
  removeTitle(project: string): string;
  removeWhat(conversations: number): string;
  removeShared: string;
  removeGo: string;
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
  remove: "删除项目…",
  removeTitle: (project) => `删除「${project}」`,
  removeWhat: (conversations) =>
    conversations ? `项目、它的记录和里面的 ${conversations} 段对话都会删掉，拿不回来。` : "项目和它的记录都会删掉，拿不回来。",
  removeShared: "共享给的人在里面的对话会移出项目。",
  removeGo: "删除",
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
  remove: "Delete project…",
  removeTitle: (project) => `Delete “${project}”`,
  removeWhat: (conversations) =>
    conversations
      ? `The project, its records and the ${conversations} conversation${conversations === 1 ? "" : "s"} in it are deleted for good.`
      : "The project and its records are deleted for good.",
  removeShared: "The conversations of those it is shared with move out of it.",
  removeGo: "Delete",
};

export interface ProjectActsOptions {
  words: ProjectActsWords;
  access: AccessWords;
  store: Store;
  confirm: Confirm;
  change(login: Login, change: AccountChange): Promise<void>;
  /** The reader's conversations in the project `town`, archived ones too. */
  conversations(town: string): ResidentNow[];
  /** `town` was deleted: a page of it is left. */
  deleted(town: string): void;
}

/** How the reader holds a project: as which login, on which hub, and
 *  whether it is theirs. */
interface Hold {
  login: Login;
  hub: HubAccounts;
  project: ProjectSeen;
  mine: boolean;
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
      ...(hold.mine ? [{ act: "transfer", icon: "move" as const, label: words.transfer }] : []),
      ...(hold.mine && hold.hub.deletes_projects ? [{ act: "delete", icon: "trash" as const, label: words.remove, danger: true }] : []),
    ];
  }

  /** Whose `town` is, where it is another's shared with the reader. */
  sharedBy(town: string): string | null {
    const hold = this.hold(town);
    return hold && !hold.mine && hold.project.granted.includes(hold.login.account) ? hold.project.owner : null;
  }

  open(town: string, name: string, anchor: HTMLElement): void {
    this.menu.open(anchor, name, this.acts(town), (act) => this.run(act, town, name));
  }

  /** Does one of its acts, chosen from this "⋯" or the sidebar's. */
  run(act: string, town: string, name: string): void {
    if (act === "share") this.share(town, name);
    else if (act === "transfer") this.transfer(town, name);
    else if (act === "delete") this.remove(town, name);
  }

  /** Deletes the project after a red card says what goes. The hub deletes
   *  it whole: each machine deletes the owner's conversations that were in
   *  it and moves the others' out, a machine away doing so when it is back.
   *  A project the hub keeps leaves every conversation as it was. */
  private remove(town: string, name: string): void {
    const hold = this.hold(town);
    if (!hold?.mine) return;
    const { words, change, store, confirm, conversations, deleted } = this.options;
    const talks = conversations(town);
    const shared = hold.project.granted.length > 0;
    confirm.ask({
      title: words.removeTitle(name),
      what: shared ? `${words.removeWhat(talks.length)}${words.removeShared}` : words.removeWhat(talks.length),
      go: words.removeGo,
      act: () => change(hold.login, { change: "delete_project", town }),
      done: () => {
        deleted(town);
        void store.refresh();
      },
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
      // The accounts it can go to, one of them chosen: a column of rows,
      // the chosen one washed, scrolling in itself where there are many.
      fields: `<fieldset class="field transfer-field"><legend class="field-label">${escape(words.transferTo)}</legend><div class="transfer-choices">${others
        .map((account, index) => `<label class="choice map-card-choice"><input type="radio" name="to" value="${escape(account.name)}"${index === 0 ? " checked" : ""}><span>${escape(account.name)}</span></label>`)
        .join("")}</div></fieldset>`,
      go: words.transferGo,
      act: async () => {
        const to = document.querySelector<HTMLInputElement>("dialog[open].confirm-sheet input[name=to]:checked")?.value;
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
      return { login, hub, project, mine: !!owner };
    }
    return null;
  }
}
