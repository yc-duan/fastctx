// The drawings of a managed machine (`world-network.md`, 受管机器): the
// three steps a code is used in, and the machine itself, one line to its
// server and others' lines coming in. Drawn with the marker; the words
// beside them stay text.

/** A screen on its stand, `w` by `h` from its top left corner. */
const screen = (x: number, y: number, w: number, h: number) =>
  `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="5"/><path d="M${x + w / 2} ${y + h} V${y + h + 8}M${x + w / 2 - 12} ${y + h + 8} H${x + w / 2 + 12}"/>`;

/** A person as a sign-in screen shows one: a head over shoulders. */
const person = (x: number, y: number) => `<circle cx="${x}" cy="${y}" r="7"/><path d="M${x - 11} ${y + 18} C${x - 9} ${y + 10} ${x + 9} ${y + 10} ${x + 11} ${y + 18}"/>`;

function art(width: number, height: number, body: string, kind = "managed-step-art"): string {
  return `<svg class="${kind}" viewBox="0 0 ${width} ${height}" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke-linecap="round" stroke-linejoin="round" stroke-width="2.6">${body}</g></svg>`;
}

/** The three steps, in order: the system account no one at the machine
 *  signs in to, chosen on its sign-in screen; Fastctx installed and open
 *  there; the code pasted where joining asks for it. */
export function managedSteps(): [string, string, string] {
  return [
    art(120, 84, `<g class="ink-graphite">${screen(14, 6, 92, 58)}${person(42, 26)}</g><g class="ink-clay">${person(78, 26)}<circle cx="78" cy="34" r="18"/></g>`),
    art(
      120,
      84,
      `<g class="ink-graphite">${screen(14, 6, 92, 58)}</g><g class="ink-clay"><circle cx="60" cy="36" r="4"/><circle cx="44" cy="22" r="6"/><circle cx="76" cy="22" r="6"/><circle cx="60" cy="52" r="5"/><path d="M49 26 L56 33M71 26 L64 33M60 40 L60 47"/></g>`,
    ),
    art(
      120,
      84,
      `<g class="ink-graphite">${screen(14, 6, 92, 58)}<rect x="24" y="24" width="56" height="16" rx="4"/></g><g class="ink-clay"><path d="M30 32 C33 28 36 36 39 32 S45 28 48 32 S54 36 57 32 S63 28 66 32 S71 35 73 32"/><path d="M85 31 L90 37 L99 25"/></g>`,
    ),
  ];
}

/** The machine itself: its one line to the server, and the lines of
 *  agents elsewhere coming in, dashed. */
export function managedMachine(kind = "managed-art"): string {
  return art(
    240,
    132,
    `<g class="ink-graphite">${screen(88, 50, 68, 46)}<circle cx="204" cy="28" r="12"/><path d="M156 58 C172 50 184 44 194 36"/><circle cx="30" cy="34" r="9"/><circle cx="26" cy="100" r="9"/></g><g class="ink-clay"><circle cx="204" cy="28" r="3.5" fill="currentColor"/><path d="M41 37 C58 44 70 56 84 64" stroke-dasharray="5 7"/><path d="M78 59 L85 65 L76 68"/><path d="M37 97 C54 92 68 86 84 82" stroke-dasharray="5 7"/><path d="M76 78 L85 82 L77 87"/></g>`,
    kind,
  );
}
