# Ink

Hand-drawn strokes for the living side of the World: machines, agents at work, things running, journals. Each stroke is one sampled centre line that the GPU expands, by instancing, into a bundle of fine translucent filaments. A fixed seed decides each filament's offset, roughness, weight and phase; time moves only the slow tremble, the dashes and the entrances and exits. One draw call per stroke, not per filament. WebGL2, no dependencies.

## Use

```ts
import { InkScene, paths } from "./ink";

const scene = new InkScene(canvas, { seed: "world-map", viewBox: [800, 360] });

scene.add("machine/ring", {
  path: paths.circle([120, 140], 24),
  style: { color: "var(--text-2)", fibers: 30, spread: 3, opacity: 0.14 },
}, { drawIn: 900 });

scene.add("machine/to-hub", {
  path: paths.cubic([180, 130], [300, 90], [350, 230], [470, 180]),
  style: { color: "var(--accent)", fibers: 10, dash: { length: 6, gap: 12, speed: 16 } },
});

scene.update("machine/to-hub", { style: { dash: null } });
scene.delete("machine/ring", { fadeOut: 600 });
scene.dispose(); // when the view goes
```

The canvas needs an explicit CSS size; its pixel buffer follows that size and the device pixel ratio. Text, hit testing and accessibility belong to the page's own DOM laid over it.

## Paths

Points are `[x, y]`; a stroke is one continuous contour. `paths.line`, `paths.polyline(points, closed)`, `paths.quadratic`, `paths.cubic`, `paths.circle(center, radius)` and `paths.arc(center, radius, start, sweep)` (radians, clockwise on screen), or a path built from `line`, `quadratic` and `cubic` segments.

## Style

`style` is a shallow patch over the defaults (or, on `update`, over the stroke's current style: pass a whole style to replace it). Lengths are in path units, CSS pixels unless a `viewBox` is given.

| Field | Default | Meaning |
|---|---:|---|
| `color` | `var(--text)` | Any CSS colour, variables included, resolved in the canvas's cascade |
| `width` | 0.55 | Width of one filament |
| `fibers` | 18 | Filaments in the bundle |
| `spread` | 2.5 | Width over which filament centres spread |
| `density` | 1.4 | Spread exponent; above 1 gathers filaments towards the centre |
| `opacity` | 0.13 | Alpha of each filament; overlaps accumulate |
| `roughness` | 0.35 | Fixed waviness |
| `jitter` | 0.22 | Largest trembling offset |
| `jitterSpeed` | 0.09 | Trembling frequency, cycles per second; 0 holds still |
| `dash` | `null` | Solid, or `{ length, gap, speed, offset? }`; positive speed flows from start to end |

## Scene

`new InkScene(canvas, { seed?, viewBox?, quality?, autoStart? })`. `add`, `update` and `delete` work by id and throw on an unknown or repeated one; `has`, `pause`, `resume`, `renderAt(seconds)` for an explicit frame, `refreshColors()`, `stats` and `dispose()`.

The clock runs only while the page is visible and the canvas on screen; a scene with nothing moving draws once. Under reduced motion, entrances complete at once, removals take effect at once, and trembling and flow hold at time zero. Theme changes through `class`, `style` or `data-theme` on the canvas or its ancestors refresh the colours by themselves. Without WebGL2 the constructor throws; there is no slower fallback.

Measured on a Ryzen 7 7800X3D with an RTX 5080: 6,000 filaments at 141 frames per second in WebView2 using about 1.2% of the machine's CPU, against 34 frames per second and 9.6% for a Canvas 2D version. A mid-range Android phone has not been measured yet.
