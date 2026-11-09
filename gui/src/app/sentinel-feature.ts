// The account selector and opt-ins above the existing watcher configuration form.
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import type { SentinelSeam } from "../world/sentinels";
import { accountFeatures } from "./account-features";
import type { FeaturesWords } from "./features-words";
import { AccountChoice, loginKey } from "./profile-account";
import { enProfile, zhProfile } from "./profile-words";
import { SentinelForm } from "./sentinel-form";
import { effect } from "./signals";
import type { Store } from "./store";

export function sentinelFeature(body: HTMLElement, store: Store, words: FeaturesWords, english: boolean,
  seam: SentinelSeam, machines: (hub: string, account: string) => { name: string; form: "desktop" | "laptop" | "server" }[]): () => void {
  body.classList.add("features-on");
  body.innerHTML = '<div class="sf-account"></div><div class="sf-holder"></div>';
  const holder = body.querySelector<HTMLElement>(".sf-holder")!;
  let shown = "";
  let generation = 0;
  const load = async (login: Login) => {
    shown = loginKey(login);
    const reading = ++generation;
    holder.textContent = words.reading;
    try {
      const view = await seam.read(login.hubName, login.account);
      if (reading !== generation) return;
      const form = new SentinelForm({ words: words.sentinels, seam,
        login: { hub: login.hubName, account: login.account },
        machines: () => machines(login.hubName, login.account),
        saved: (saved) => accountFeatures.saved(login, saved.settings),
      });
      accountFeatures.saved(login, view.settings);
      form.show(view);
      holder.replaceChildren(form.element);
    } catch (error) {
      if (reading !== generation) return;
      holder.innerHTML = `<p class="feature-problem">${escape(String(error))}</p><button class="text-button sf-retry">${escape(words.retry)}</button>`;
      holder.querySelector(".sf-retry")!.addEventListener("click", () => void load(login));
    }
  };
  const choice = new AccountChoice(english ? enProfile : zhProfile, (login) => void load(login));
  body.querySelector(".sf-account")!.append(choice.element);
  const stop = effect(() => {
    const world = store.world.get();
    choice.draw(world);
    const login = choice.chosen();
    if (login && loginKey(login) !== shown) void load(login);
    if (!login) { ++generation; shown = ""; holder.textContent = words.noAccount; }
  });
  return () => { ++generation; stop(); choice.leave(); };
}
