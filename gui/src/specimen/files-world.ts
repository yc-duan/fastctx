// The specimen machines' files: a few folders on each, what the files in
// them hold, and the lines of the changes the specimen's activity tells
// of, made up so that they add to the counts told. Seeded, so every
// screenshot shows the same.

import type { DiffRow } from "../conversation/diff";
import type { Listing, TreeEntry } from "../files/tree";
import type { Places } from "../files/page";
import { MemoryLines } from "../longview";
import { seeded } from "../pen/shapes";

const RUST = (name: string) => [
  `//! ${name}: fixed-point control for the Mark 50 joint drive.`,
  "",
  "use crate::q16::Q16;",
  "use crate::telemetry::Frame;",
  "",
  "/// Gains of one loop, in Q16.16.",
  "#[derive(Clone, Copy, Debug)]",
  "pub struct Gains {",
  "    pub kp: Q16,",
  "    pub ki: Q16,",
  "    pub kd: Q16,",
  "}",
  "",
  "pub struct Controller {",
  "    gains: Gains,",
  "    integral: Q16,",
  "    last_error: Q16,",
  "    limit: Q16,",
  "}",
  "",
  "impl Controller {",
  "    pub fn new(gains: Gains, limit: Q16) -> Self {",
  "        Self { gains, integral: Q16::ZERO, last_error: Q16::ZERO, limit }",
  "    }",
  "",
  "    /// One step of the loop: the error in, the drive out, clamped.",
  "    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {",
  "        let p = self.gains.kp * error;",
  "        self.integral = (self.integral + self.gains.ki * error * dt).clamp(-self.limit, self.limit);",
  "        let d = self.gains.kd * (error - self.last_error) / dt;",
  "        self.last_error = error;",
  "        (p + self.integral + d).clamp(-self.limit, self.limit)",
  "    }",
  "",
  "    pub fn reset(&mut self) {",
  "        self.integral = Q16::ZERO;",
  "        self.last_error = Q16::ZERO;",
  "    }",
  "",
  "    pub fn frame(&self) -> Frame {",
  "        Frame::control(self.integral, self.last_error)",
  "    }",
  "}",
  "",
  "#[cfg(test)]",
  "mod tests {",
  "    use super::*;",
  "",
  "    #[test]",
  "    fn holds_at_the_limit() {",
  "        let mut loop_ = Controller::new(Gains { kp: Q16::ONE, ki: Q16::ONE, kd: Q16::ZERO }, Q16::from_int(4));",
  "        for _ in 0..1000 {",
  "            assert!(loop_.step(Q16::from_int(9), Q16::ONE) <= Q16::from_int(4));",
  "        }",
  "    }",
  "}",
];

const TOML = [
  "[package]",
  'name = "firmware"',
  'version = "0.9.2"',
  'edition = "2021"',
  "",
  "[dependencies]",
  'heapless = "0.8"',
  'fixed = "1.28"',
  "",
  "[profile.release]",
  'opt-level = "s"',
  "lto = true",
];

const NOTES = [
  "# 控制环",
  "",
  "关节驱动的 PID 环从浮点改成 **Q16.16 定点**，原因有三：",
  "",
  "1. 目标芯片没有硬件浮点，软浮点每步多花约 40 µs；",
  "2. 积分项在浮点下的舍入让长时间保持时有缓慢漂移；",
  "3. 定点的溢出点是确定的，可以逐个测试。",
  "",
  "## 步骤",
  "",
  "- [x] `Q16` 类型与饱和运算",
  "- [x] 控制器改写，行为与浮点版逐步对比",
  "- [ ] 台架上跑满 2 小时保持测试",
  "",
  "```rust",
  "let drive = controller.step(error, dt);",
  "```",
  "",
  "| 项 | 浮点 | 定点 |",
  "| --- | --- | --- |",
  "| 每步耗时 | 61 µs | 19 µs |",
  "| 2 小时漂移 | 0.8° | 0° |",
  "",
  "取舍的来由见 `src/pid/controller.rs` 顶部的说明。",
];

const TSX = [
  'import { useEffect, useState } from "react";',
  'import { Chart } from "./Chart";',
  "",
  "export function App() {",
  "  const [frames, setFrames] = useState<Frame[]>([]);",
  "  useEffect(() => {",
  '    const feed = new EventSource("/frames");',
  "    feed.onmessage = (event) => setFrames((all) => [...all.slice(-599), JSON.parse(event.data)]);",
  "    return () => feed.close();",
  "  }, []);",
  "  return (",
  '    <main className="dashboard">',
  "      <h1>Mark 50</h1>",
  "      <Chart frames={frames} />",
  "    </main>",
  "  );",
  "}",
];

const CONFIG = ["[ingest]", 'listen = "0.0.0.0:7400"', "batch = 5000", "retry = 3", "", "[spool]", 'dir = "/var/spool/telemetry"', "max_bytes = 2_000_000_000"];

const PYTHON = [
  "import argparse",
  "import torch",
  "",
  "def main() -> None:",
  "    parser = argparse.ArgumentParser()",
  '    parser.add_argument("--epochs", type=int, default=3)',
  '    parser.add_argument("--batch", type=int, default=64)',
  "    args = parser.parse_args()",
  "    model = build()",
  "    for epoch in range(args.epochs):",
  "        loss = train_one(model, args.batch)",
  '        print(f"epoch {epoch + 1}: loss {loss:.4f}")',
  '        torch.save(model.state_dict(), f"checkpoints/epoch-{epoch + 1}.pt")',
  "",
  'if __name__ == "__main__":',
  "    main()",
];

/** What a file holds, by what it is. */
function content(path: string): string[] {
  const name = path.split(/[\\/]/).pop() ?? path;
  const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  if (ext === "rs") return RUST(name.replace(/\.rs$/, ""));
  if (ext === "toml") return name === "config.toml" ? CONFIG : TOML;
  if (ext === "md") return NOTES;
  if (ext === "tsx" || ext === "ts") return TSX;
  if (ext === "py") return PYTHON;
  if (ext === "json") return ["{", '  "name": "dashboard",', '  "private": true,', '  "scripts": { "build": "vite build", "dev": "vite" }', "}"];
  if (ext === "html") return ["<!doctype html>", '<html lang="en">', "  <head>", '    <meta charset="utf-8">', "    <title>Mark 50</title>", '    <script type="module" src="/assets/index-4f1c.js"></script>', "  </head>", '  <body><div id="root"></div></body>', "</html>"];
  if (ext === "csv") return ["at,joint,angle,drive", ...Array.from({ length: 40 }, (_, n) => `2026-11-16T09:${String(n).padStart(2, "0")}:00Z,3,${(12 + Math.sin(n / 4) * 3).toFixed(3)},${(0.4 + Math.cos(n / 5) * 0.2).toFixed(3)}`)];
  if (ext === "yaml" || ext === "yml") return ["model:", "  width: 512", "  depth: 8", "train:", "  lr: 3.0e-4", "  warmup: 500"];
  return [`${name}`, "", "Notes kept beside the work."];
}

interface Machine {
  home: string;
  places: { name: string; path: string }[];
  files: string[];
}

const MACHINES: Record<string, Machine> = {
  desktop: {
    home: "C:/Users/me",
    places: [
      { name: "C:", path: "C:/" },
      { name: "D:", path: "D:/" },
    ],
    files: [
      "C:/Users/me/.cargo/config.toml",
      "C:/Users/me/Documents/采购清单.md",
      "C:/Users/me/mark50/README.md",
      "C:/Users/me/mark50/firmware/Cargo.toml",
      "C:/Users/me/mark50/firmware/README.md",
      "C:/Users/me/mark50/firmware/src/main.rs",
      "C:/Users/me/mark50/firmware/src/q16.rs",
      "C:/Users/me/mark50/firmware/src/radio.rs",
      "C:/Users/me/mark50/firmware/src/pid/controller.rs",
      "C:/Users/me/mark50/firmware/src/pid/mod.rs",
      "C:/Users/me/mark50/firmware/src/telemetry/frame.rs",
      "C:/Users/me/mark50/firmware/src/telemetry/mod.rs",
      "C:/Users/me/mark50/firmware/tests/pid_fixed.rs",
      "C:/Users/me/mark50/dashboard/package.json",
      "C:/Users/me/mark50/dashboard/src/App.tsx",
      "C:/Users/me/mark50/dashboard/src/Chart.tsx",
      "C:/Users/me/mark50/dashboard/dist/index.html",
      "C:/Users/me/mark50/notes/控制环.md",
      "C:/Users/me/mark50/notes/bringup.pdf",
      "C:/Users/me/mark50/notes/step-response.png",
      "D:/data/telemetry-2026-11.parquet",
      "D:/models/joint-v3.onnx",
    ],
  },
  server: {
    home: "/home/ops",
    places: [{ name: "/", path: "/" }],
    files: ["/home/ops/.bashrc", "/srv/telemetry/README.md", "/srv/telemetry/ingest/config.toml", "/srv/telemetry/ingest/src/main.rs", "/srv/telemetry/ingest/src/retry.rs", "/srv/telemetry/ingest/src/spool.rs", "/srv/telemetry/out/day.csv"],
  },
  gpu: {
    home: "/home/bo",
    places: [{ name: "/", path: "/" }],
    files: ["/home/bo/runs/train.py", "/home/bo/runs/config.yaml", "/home/bo/runs/checkpoints/epoch-1.pt", "/home/bo/runs/checkpoints/epoch-2.pt", "/home/bo/runs/checkpoints/epoch-3.pt"],
  },
  laptop: {
    home: "/home/me",
    places: [{ name: "/", path: "/" }],
    files: [
      "C:/Users/me/mark50/firmware/Cargo.toml",
      "C:/Users/me/mark50/firmware/README.md",
      "C:/Users/me/mark50/firmware/src/main.rs",
      "C:/Users/me/mark50/firmware/src/radio.rs",
      "C:/Users/me/mark50/firmware/src/pid/controller.rs",
      "C:/Users/me/mark50/firmware/src/pid/mod.rs",
      "C:/Users/me/mark50/firmware/tests/pid_fixed.rs",
      "/home/me/mark50/README.md",
      "/home/me/mark50/Cargo.toml",
      "/home/me/mark50/docs/控制环.md",
      "/home/me/mark50/docs/step-response.png",
      "/home/me/mark50/src/main.rs",
      "/home/me/mark50/src/ctrl/fixed.rs",
      "/home/me/mark50/src/ctrl/mod.rs",
      "/home/me/mark50/src/ctrl/q16.rs",
      "/home/me/mark50/src/ctrl/tests/step_vectors.rs",
      "/home/me/data/day.csv",
      "/srv/ingest/src/retry.rs",
      "/srv/ingest/src/writer.rs",
    ],
  },
};

const bare = (path: string) => (path.length > 1 && path.endsWith("/") && !/^[A-Za-z]:\/$/.test(path) ? path.slice(0, -1) : path);
const wait = <T>(value: T, ms = 160) => new Promise<T>((resolve) => setTimeout(() => resolve(value), ms));

export function specimenPlaces(machine: string): Promise<Places> {
  const held = MACHINES[machine];
  return wait(held ? { home: held.home, places: held.places } : { home: null, places: [] });
}

/** What the folder at `path` holds on a specimen machine. */
export function specimenList(machine: string, path: string): Promise<Listing> {
  const held = MACHINES[machine];
  if (!held) return Promise.reject(new Error(`${machine} did not answer`));
  const folder = bare(path);
  const prefix = folder.endsWith("/") ? folder : `${folder}/`;
  const entries = new Map<string, TreeEntry>();
  const random = seeded(`files:${machine}`);
  for (const file of held.files) {
    if (!file.startsWith(prefix)) continue;
    const rest = file.slice(prefix.length);
    const name = rest.split("/")[0]!;
    const inner = rest.includes("/");
    if (entries.has(name)) continue;
    entries.set(name, { name, path: `${prefix}${name}`, kind: inner ? "folder" : "file", hidden: name.startsWith("."), ...(inner ? {} : { size: Math.floor(400 + random() * 90_000), modified: Date.now() - Math.floor(random() * 30 * 86_400_000) }) });
  }
  const sorted = [...entries.values()].sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "folder" ? -1 : 1));
  const up = folder.slice(0, folder.lastIndexOf("/"));
  return wait({ path, parent: /^[A-Za-z]:\/$/.test(folder) || folder === "/" ? null : up || "/", entries: sorted });
}

/** A specimen file's lines, or none where the machine has no such file. */
export function specimenFileText(machine: string, path: string): string[] | null {
  return MACHINES[machine]?.files.includes(path) ? content(path) : null;
}

export function specimenFileLines(machine: string, path: string): MemoryLines & { close(): Promise<void> } {
  const lines = specimenFileText(machine, path);
  if (!lines) throw { message: "the source does not exist", code: "source_missing" };
  return Object.assign(new MemoryLines(lines, true), { close: async () => {} });
}

/** A picture a specimen machine holds: a step response, drawn. */
export function specimenFilePicture(): Promise<string> {
  const points = Array.from({ length: 60 }, (_, n) => {
    const t = n / 59;
    const y = 1 - Math.exp(-6 * t) * Math.cos(14 * t);
    return `${(40 + t * 520).toFixed(1)},${(300 - y * 200).toFixed(1)}`;
  }).join(" ");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="600" height="360" viewBox="0 0 600 360"><rect width="600" height="360" fill="#f6f1e6"/><g stroke="#b9ae97" stroke-width="1">${[100, 150, 200, 250, 300].map((y) => `<line x1="40" x2="560" y1="${y}" y2="${y}"/>`).join("")}</g><line x1="40" x2="560" y1="100" y2="100" stroke="#bf5d3a" stroke-dasharray="6 5" stroke-width="1.5"/><polyline points="${points}" fill="none" stroke="#3b3a36" stroke-width="2.5" stroke-linejoin="round"/><text x="40" y="44" font-family="sans-serif" font-size="20" fill="#3b3a36">Step response, joint 3</text><text x="40" y="340" font-family="sans-serif" font-size="13" fill="#7a735f">0 ms</text><text x="520" y="340" font-family="sans-serif" font-size="13" fill="#7a735f">400 ms</text></svg>`;
  return wait(`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`, 220);
}

/** The lines of a change that added `added` and removed `removed` lines of
 *  the file at `path`, made up from what the file holds. */
export function specimenRows(path: string, how: "added" | "changed" | "removed" | "copied", added: number, removed: number, seed: string): Promise<DiffRow[] | null> {
  if (!added && !removed) return wait(null);
  const lines = content(path);
  const at = (index: number) => lines[index % lines.length]!;
  const rows: DiffRow[] = [];
  if (how === "added" || how === "removed") {
    const count = how === "added" ? added : removed;
    for (let n = 0; n < count; n += 1) rows.push(how === "added" ? { kind: "add", old: null, new: n + 1, text: at(n) } : { kind: "del", old: n + 1, new: null, text: at(n) });
    return wait(rows, 240);
  }
  const random = seeded(`rows:${seed}`);
  // Two or three parts of the file, each a few lines taken out and a few
  // put in, the lines round them left alone.
  const parts = Math.max(1, Math.min(3, Math.ceil((added + removed) / 12)));
  let old = 1 + Math.floor(random() * 6);
  let next = old;
  let leftAdded = added;
  let leftRemoved = removed;
  for (let part = 0; part < parts; part += 1) {
    const last = part === parts - 1;
    const takes = last ? leftRemoved : Math.min(leftRemoved, Math.ceil(removed / parts));
    const puts = last ? leftAdded : Math.min(leftAdded, Math.ceil(added / parts));
    leftRemoved -= takes;
    leftAdded -= puts;
    if (part > 0) {
      const gap = 8 + Math.floor(random() * 30);
      rows.push({ kind: "gap", old: null, new: null, text: "", hidden: gap });
      old += gap;
      next += gap;
    } else if (old > 1) rows.push({ kind: "gap", old: null, new: null, text: "", hidden: old - 1 });
    for (let n = 0; n < 3; n += 1) rows.push({ kind: "same", old: old++, new: next++, text: at(old + 3) });
    for (let n = 0; n < takes; n += 1) rows.push({ kind: "del", old: old++, new: null, text: at(old + 7).replace(/Q16/g, "f32").replace(/clamp\(/, "min(") });
    for (let n = 0; n < puts; n += 1) rows.push({ kind: "add", old: null, new: next++, text: at(next + 7) });
    for (let n = 0; n < 3; n += 1) rows.push({ kind: "same", old: old++, new: next++, text: at(old + 11) });
  }
  return wait(rows, 240);
}
