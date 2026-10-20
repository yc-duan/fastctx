// Hover belongs to a pointer that can rest on things. Every style rule whose
// selector hovers applies only where the main pointer can (`@media (hover:
// hover)`): on a phone a tap would otherwise leave what it touched lit, and
// what hover reveals would come up under the finger (`world-gui.md`, 界面的结构).
// A hover that only changes colours is given to the pressed state on a touch
// screen instead, so a press still answers where it lands.
//
// Rules already written for one kind of pointer (inside a media query that
// asks about hover or the pointer) are left as written.

/** What a press may borrow from a hover: colours, nothing that shows,
 *  hides, moves or resizes. */
const COLOURS = new Set(["background", "background-color", "color", "fill", "stroke"]);

const hovers = (selector) => selector.includes(":hover");

function forOnePointer(rule) {
  for (let parent = rule.parent; parent; parent = parent.parent) {
    if (parent.type === "atrule" && parent.name === "media" && /hover|pointer/.test(parent.params)) return true;
  }
  return false;
}

export function hoverOnly() {
  return {
    postcssPlugin: "fastctx-hover-only",
    OnceExit(root, { AtRule }) {
      const found = [];
      root.walkRules((rule) => {
        if (hovers(rule.selector) && !forOnePointer(rule)) found.push(rule);
      });
      for (const rule of found) {
        const hovering = rule.selectors.filter(hovers);
        const rest = rule.selectors.filter((selector) => !hovers(selector));
        let last = rule;
        const hover = new AtRule({ name: "media", params: "(hover: hover)" });
        hover.append(rule.clone({ selectors: hovering }));
        last.after(hover);
        last = hover;
        const decls = rule.nodes.filter((node) => node.type === "decl");
        if (decls.length && decls.every((decl) => COLOURS.has(decl.prop))) {
          const press = new AtRule({ name: "media", params: "(hover: none)" });
          press.append(rule.clone({ selectors: hovering.map((selector) => selector.replaceAll(":hover", ":active")) }));
          last.after(press);
        }
        if (rest.length) rule.selectors = rest;
        else rule.remove();
      }
    },
  };
}
hoverOnly.postcss = true;
