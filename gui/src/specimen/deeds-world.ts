// The World's own doings in the deeds specimen (`deeds.ts`): a model
// trained on one machine in the background and waited for, its weights
// copied to another, a plot and a page of a datasheet looked at, a page
// published, a stray job stopped, the plan written, a sentinel set and the
// goal completed. Each prints what the World prints for it, and carries
// the steps the World told of it: the `on` that sent it, and what it did in
// the job that `on` started.

import { run, type Script } from "./script";
import type { Content, Speaker, Tool } from "../world/events";
import type { RunStep, StepDid } from "../world/run-facts";

let told = 0;

/** `tool`, a run of `on laptop <command>`, with what the World told of it:
 *  the `on`, and `did` in the job it sent. */
function stepped<T extends Partial<Tool>>(tool: T, did: StepDid | null, end: RunStep["began"] = "2026-10-04T15:20:00Z"): T {
  told += 1;
  const sent = `j-s${told.toString(16).padStart(4, "0")}`;
  const command = tool.does?.as === "command" ? tool.does.command.replace(/^on\s+\S+\s+/, "") : "";
  const steps: RunStep[] = [
    { id: `on-${told}`, job: tool.run?.job ?? "j-run", machine: "laptop", began: end, ended: end, kind: "on", to: "laptop", command, sent, end: { kind: "exited", code: 0 } },
  ];
  if (did) steps.push({ id: `step-${told}`, job: sent, machine: "laptop", began: end, ended: end, ...did });
  return { ...tool, ...(tool.run ? { run: { ...tool.run, steps } } : {}) };
}

/** A plot, drawn as a picture the World would have given. */
function plot(stroke: string, settle: string): Content {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="300" viewBox="0 0 480 300"><rect width="480" height="300" fill="#f4f1ea"/><g stroke="#d8d2c4" stroke-width="1">${Array.from({ length: 11 }, (_, i) => `<line x1="${i * 48}" y1="0" x2="${i * 48}" y2="300"/>`).join("")}${Array.from({ length: 7 }, (_, i) => `<line x1="0" y1="${i * 50}" x2="480" y2="${i * 50}"/>`).join("")}</g><path d="${stroke}" fill="none" stroke="#3a3a36" stroke-width="2.2"/><path d="${settle}" fill="none" stroke="#d97757" stroke-width="2.2"/></svg>`;
  return { type: "image_url", url: `data:image/svg+xml;base64,${btoa(svg)}` };
}

const page = (): Content => {
  const lines = Array.from({ length: 14 }, (_, i) => `<rect x="40" y="${90 + i * 22}" width="${200 + ((i * 53) % 140)}" height="6" rx="3" fill="#b9b3a6"/>`).join("");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="424" viewBox="0 0 300 424"><rect width="300" height="424" fill="#ffffff"/><rect x="40" y="40" width="160" height="14" rx="4" fill="#3a3a36"/>${lines}<rect x="40" y="410" width="220" height="1" fill="#d8d2c4"/></svg>`;
  return { type: "image_url", url: `data:image/svg+xml;base64,${btoa(svg)}` };
};

export function worldWork(s: Script, user: Speaker): void {
  s.heard("t4", user, "gpu 上把前馈模型再训 400 轮，训完拷到 laptop 接进控制环，画个阶跃响应给我看，再把调参面板发出来。");
  s.started("t4");
  s.say("t4", "thinking", "**Training goes on by itself; the rest waits for it**", 2);
  s.tool("t4", stepped(run("on laptop fastctx project enter mark50", "=== run on laptop (exited 0) ===", "=== you work in the project mark50 from now on ===", { duration_ms: 300 }), { kind: "project", town: "mark50" }), 1);
  s.tool(
    "t4",
    {
      ...run("on gpu python train.py --model ff --epochs 400 --out runs/ff-1004", "=== run on gpu (started; j-k3x9a2) ===", "epoch 1/400 loss 0.412", { duration_ms: 1_200 }, "gpu"),
      does: { as: "command", command: "on gpu python train.py --model ff --epochs 400 --out runs/ff-1004", cwd: null, exit: null, job: "j-k3x9a2" },
    },
    1,
  );
  s.tool("t4", stepped(run("on laptop fastctx stop j-p0q1r2", "=== run on laptop (exited 0) ===", "=== stop j-p0q1r2 on lab (stopped) ===", { duration_ms: 400 }), { kind: "stop", what: "j-p0q1r2", on: "lab", how: "stopped" }), 1);
  s.tool("t4", stepped(run("on laptop fastctx sense '训练的验证误差降到 0.005 以下' --tell '可以停了，接进控制环'", "=== run on laptop (exited 0) ===", "", { duration_ms: 300 }), { kind: "sense", sentinel: "s-41c2", condition: "训练的验证误差降到 0.005 以下" }), 1);
  s.say("t4", "thinking", "**Waiting for the run on gpu**", 2);
  s.tool(
    "t4",
    stepped(
      run(
        "on laptop fastctx wait j-k3x9a2",
        "=== run on laptop (exited 0) ===",
        ["=== j-k3x9a2 on gpu ended (exit 0) 3 minutes ago; its output is in /home/bo/.fastctx/world/out/j-k3x9a2.log ===", "=== its last lines ===", "epoch 399/400 loss 0.0071 val 0.0049", "epoch 400/400 loss 0.0069 val 0.0048", "saved runs/ff-1004/ff.onnx (4.2 MB)"].join("\n"),
        { duration_ms: 186_000 },
      ),
      {
        kind: "wait",
        jobs: [
          {
            job: "j-k3x9a2",
            machine: "gpu",
            at: "2026-10-04T15:17:00Z",
            end: { kind: "exited", code: 0 },
            tail: ["epoch 399/400 loss 0.0071 val 0.0049", "epoch 400/400 loss 0.0069 val 0.0048", "saved runs/ff-1004/ff.onnx (4.2 MB)"].join("\n"),
            command: "python train.py --model ff --epochs 400 --out runs/ff-1004",
          },
        ],
      },
    ),
    1,
  );
  s.tool(
    "t4",
    stepped(
      run(
        "on laptop fastctx copy gpu:/home/bo/runs/ff-1004/ff.onnx gpu:/home/bo/runs/ff-1004/meta.json gpu:/home/bo/runs/ff-1004/ckpt /srv/mark50/models/",
        "=== run on laptop (exited 1) ===",
        ["=== copy gpu:/home/bo/runs/ff-1004 -> laptop:/srv/mark50/models/ (2 files, 4.3 MB; 1 already the same; 1 not copied) ===", "not copied: /home/bo/runs/ff-1004/ckpt: a directory; copy it with a trailing /"].join("\n"),
        { duration_ms: 2_400 },
      ),
      {
        kind: "copy",
        from: [
          { machine: "gpu", path: "/home/bo/runs/ff-1004/ff.onnx" },
          { machine: "gpu", path: "/home/bo/runs/ff-1004/meta.json" },
        ],
        to: { machine: "laptop", path: "/srv/mark50/models/" },
        files: 2,
        bytes: 4_508_877,
        unchanged: 1,
        problems: ["/home/bo/runs/ff-1004/ckpt: a directory; copy it with a trailing /"],
      },
    ),
    1,
  );
  s.tool(
    "t4",
    stepped({
      ...run(
        "on laptop fastctx view --image /srv/mark50/plots/step.png /srv/mark50/plots/step-before.png",
        "=== run on laptop (exited 0) ===",
        ["=== view (2 files: 2 shown) ===", "=== laptop:/srv/mark50/plots/step.png (image 1200x750, image/png) ===", "=== laptop:/srv/mark50/plots/step-before.png (image 1200x750, image/png) ==="].join("\n"),
        { duration_ms: 500 },
      ),
      output: [
        { type: "text", text: "=== run on laptop (exited 0) ===\n=== view (2 files: 2 shown) ===" },
        plot("M0 280 C 40 280 60 60 120 80 S 200 150 260 120 S 340 110 480 112", "M0 120 L480 120"),
        plot("M0 280 C 30 280 40 20 80 40 S 130 210 170 180 S 220 40 260 70 S 330 160 380 130 S 440 100 480 118", "M0 120 L480 120"),
      ],
    }, {
      kind: "view",
      paths: ["/srv/mark50/plots/step.png", "/srv/mark50/plots/step-before.png"],
      pictures: [
        { path: "/srv/mark50/plots/step.png", width: 1200, height: 750, mime: "image/png" },
        { path: "/srv/mark50/plots/step-before.png", width: 1200, height: 750, mime: "image/png" },
      ],
    }),
    1,
  );
  s.tool(
    "t4",
    stepped({
      ...run(
        "on laptop fastctx view --pages 7 --image /srv/mark50/docs/joint-driver.pdf",
        "=== run on laptop (exited 0) ===",
        ["=== view (1 file: 1 shown) ===", "=== laptop:/srv/mark50/docs/joint-driver.pdf (pdf, 24 pages; page 7 shown) ===", "=== page 7 (image 1414x2000, image/png) ==="].join("\n"),
        { duration_ms: 900 },
      ),
      output: [{ type: "text", text: "=== run on laptop (exited 0) ===" }, page()],
    }, {
      kind: "view",
      paths: ["/srv/mark50/docs/joint-driver.pdf"],
      pictures: [{ path: "/srv/mark50/docs/joint-driver.pdf", page: 7, width: 1414, height: 2000, mime: "image/png" }],
    }),
    1,
  );
  s.tool("t4", stepped(run("on laptop fastctx page publish tuning --file /srv/mark50/pages/tuning.html --title 调参面板 --run laptop 'cargo run --bin tune'", "=== run on laptop (exited 0) ===", "=== page tuning · 调参面板 on laptop, version 3 ===", { duration_ms: 600 }), { kind: "page", name: "tuning", town: "mark50", title: "调参面板", version: 3 }), 1);
  s.tool("t4", stepped(run("on laptop fastctx plan --file /srv/mark50/.plan.md", "=== run on laptop (exited 0) ===", "Plan version 6.", { duration_ms: 200 }), { kind: "plan", version: 6 }), 1);
  s.tool("t4", stepped(run("on laptop fastctx goal complete --file /srv/mark50/.done.md", "=== run on laptop (exited 0) ===", '{"state":"complete"}', { duration_ms: 200 }), { kind: "goal", mark: "complete" }), 1);
  s.say("t4", "message", "训完了：400 轮，验证误差 0.0048，模型拷到了 laptop 的 `/srv/mark50/models/`（`ckpt` 是个目录，没跟着过来，用不上）。阶跃响应在上面，超调从 18% 降到 3%。调参面板发成了第 3 版。lab 上那个挂了一夜的旧作业顺手停了。", 2);
  s.ended("t4", "completed", null, 1);
}
