// Drawing a part again as the World or the clock moves changes only what
// differs (`world-gui.md`, 界面的流畅): the new markup is laid over the old,
// node by node, and what is the same is left as it is, with its hover, its
// focus, its scroll, its running animation; a row whose time said moved
// gets that word, not a new list. Children named by `data-key` (or an id)
// are matched by name wherever they moved; the rest by place and kind.

/** Makes `target`'s children `html`, changing only what differs. */
export function morph(target: Element, html: string): void {
  const template = document.createElement("template");
  template.innerHTML = html;
  children(target, template.content);
}

function keyOf(node: Node): string | null {
  if (!(node instanceof Element)) return null;
  return node.getAttribute("data-key") ?? (node.id || null);
}

/** Whether two elements stand for the same thing, by what names it: their
 *  data attributes, where they link, their name and id (as `app/focus.ts`
 *  finds a control again). */
function sameIdentity(a: Element, b: Element): boolean {
  const naming = (name: string) => (name.startsWith("data-") && !name.startsWith("data-world-")) || name === "href" || name === "name" || name === "id";
  const names = new Set([...a.getAttributeNames(), ...b.getAttributeNames()].filter(naming));
  for (const name of names) if (a.getAttribute(name) !== b.getAttribute(name)) return false;
  return true;
}

function sameKind(a: Node, b: Node): boolean {
  return a.nodeType === b.nodeType && (!(a instanceof Element) || a.tagName === (b as Element).tagName);
}

function children(from: Node, to: Node): void {
  const fresh = Array.from(to.childNodes);
  const named = new Map<string, ChildNode>();
  for (let child = from.firstChild; child; child = child.nextSibling) {
    const key = keyOf(child);
    if (key !== null && !named.has(key)) named.set(key, child);
  }
  let at: ChildNode | null = from.firstChild;
  for (const next of fresh) {
    const key = keyOf(next);
    let match: ChildNode | null = null;
    if (key !== null) {
      match = named.get(key) ?? null;
      if (match && !sameKind(match, next)) match = null;
      if (match) named.delete(key);
    } else if (at && keyOf(at) === null && sameKind(at, next)) {
      match = at;
    }
    if (!match) {
      from.insertBefore(next, at);
      continue;
    }
    if (match === at) at = at.nextSibling;
    else from.insertBefore(match, at);
    node(match, next);
  }
  while (at) {
    const after: ChildNode | null = at.nextSibling;
    from.removeChild(at);
    at = after;
  }
}

function node(old: ChildNode, next: ChildNode): void {
  if (old.isEqualNode(next)) return;
  if (!(old instanceof Element) || !(next instanceof Element)) {
    if (old.nodeValue !== next.nodeValue) old.nodeValue = next.nodeValue;
    return;
  }
  // What the person is typing into is theirs until they leave it.
  if (old === document.activeElement && (old instanceof HTMLInputElement || old instanceof HTMLTextAreaElement || old instanceof HTMLSelectElement)) return;
  // What holds focus, or holds what does, now drawn as another thing is
  // not made into it: focus would stay on the other, and a key pressed act
  // on it. It is put in its place whole, and focus found again where the
  // thing it was on went (`app/focus.ts`).
  if (old.contains(document.activeElement) && !sameIdentity(old, next)) {
    old.replaceWith(next);
    return;
  }
  for (const { name } of Array.from(old.attributes)) if (!next.hasAttribute(name)) old.removeAttribute(name);
  for (const { name, value } of Array.from(next.attributes)) if (old.getAttribute(name) !== value) old.setAttribute(name, value);
  if (old instanceof HTMLInputElement && next instanceof HTMLInputElement) {
    if (old.value !== next.value) old.value = next.value;
    if (old.checked !== next.checked) old.checked = next.checked;
  }
  children(old, next);
}
