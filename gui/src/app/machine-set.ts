// A setting of a machine in its page's 「管理」, as an application's settings
// show one: what it is, a line of why where it needs one, and at the end its
// value and what can be done with it.

/** One setting; `name`, `said` and `end` are markup already. */
export function setRow(name: string, said: string, end: string): string {
  return `<div class="mp-set"><div class="mp-set-what"><span class="mp-set-name">${name}</span>${
    said ? `<span class="mp-set-said">${said}</span>` : ""
  }</div>${end ? `<div class="mp-set-end">${end}</div>` : ""}</div>`;
}
