// The first screen while no language is chosen (`world-gui.md`, 第一次打开):
// the languages the interface speaks, each said in itself, the system's
// first and ready to be chosen with Enter; asked in both, as nothing tells
// yet which the reader reads. Drawn before the rest of the interface loads.

import "../design/fonts.css";
import "../design/tokens.css";
import "../design/base.css";
import "./first-language.css";
import type { Language } from "../language";

const NAMES: Record<Language, string> = { zh: "中文", en: "English" };

/** The network's mark, as the sidebar draws it, larger. */
const MARK = `<svg class="first-mark" width="44" height="44" viewBox="0 0 22 22" aria-hidden="true"><g fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"><circle cx="11" cy="11" r="2"/><circle cx="4.5" cy="6" r="2.6"/><circle cx="17.5" cy="6.5" r="2.6"/><circle cx="11" cy="18" r="2.6"/><path d="M6.6 7.4L9.3 9.7M15.4 7.8l-2.7 1.9M11 13.1v2.3"/></g></svg>`;

/** Shows the languages until one is chosen. */
export function askLanguage(first: Language): Promise<Language> {
  const order: Language[] = first === "zh" ? ["zh", "en"] : ["en", "zh"];
  const page = document.createElement("main");
  page.className = "first-language";
  page.innerHTML = `${MARK}
    <h1 class="first-title" lang="zh-CN">选择语言 <span lang="en">· Choose a language</span></h1>
    <div class="first-choices" role="radiogroup" aria-label="Language / 语言">${order
      .map((language) => `<button class="first-choice" type="button" role="radio" aria-checked="false" data-language="${language}" lang="${language === "zh" ? "zh-CN" : "en"}">${NAMES[language]}</button>`)
      .join("")}</div>`;
  const app = document.getElementById("app");
  if (app) app.replaceChildren(page);
  else document.body.append(page);
  const buttons = [...page.querySelectorAll<HTMLButtonElement>(".first-choice")];
  buttons[0]?.focus();
  return new Promise((resolve) => {
    page.addEventListener("click", (event) => {
      const chosen = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-language]");
      if (!chosen) return;
      chosen.setAttribute("aria-checked", "true");
      page.remove();
      resolve(chosen.dataset.language as Language);
    });
    // The arrows go from one to the other, as in a list.
    page.addEventListener("keydown", (event) => {
      const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (event.key === "ArrowDown" || event.key === "ArrowRight") buttons[Math.min(buttons.length - 1, at + 1)]?.focus();
      else if (event.key === "ArrowUp" || event.key === "ArrowLeft") buttons[Math.max(0, at - 1)]?.focus();
      else return;
      event.preventDefault();
    });
  });
}
