/** Facts in a row, parted by a dot that never ends or begins a line: each
 *  fact carries the dot before it, and the one a line would begin with
 *  falls outside the row and is not drawn. The facts are markup, escaped by
 *  the caller; a fact keeps together unless it is wider than the row. */
export function factRow(facts: string[], className = ""): string {
  const items = facts.map((fact) => `<span class="fact-row-item">${fact}</span>`).join("");
  return `<span class="fact-row${className ? ` ${className}` : ""}"><span class="fact-row-in">${items}</span></span>`;
}
