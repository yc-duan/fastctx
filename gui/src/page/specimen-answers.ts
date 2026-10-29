// Made-up but plausible answers of the World's tools, for looking at a page
// without a World: the page specimen's stand-in host, and the interface's
// specimen when it opens a page.

const steps = Array.from({ length: 60 }, (_, i) => i * 50);
const metrics = steps.map((step, i) => ({
  step,
  loss: Number((2.3 * Math.exp(-i / 14) + 0.38 + Math.sin(i / 3) * 0.02).toFixed(4)),
  val_loss: Number((2.35 * Math.exp(-i / 16) + 0.41 + Math.cos(i / 4) * 0.025).toFixed(4)),
}));
const log = metrics
  .slice(-14)
  .map((m) => `step ${String(m.step).padStart(4)}  loss ${m.loss.toFixed(4)}  val ${m.val_loss.toFixed(4)}  lr 3.0e-4  12.4 it/s`)
  .join("\n");

export const answers: Record<string, (args: Record<string, unknown>) => unknown> = {
  "world.now": () => ({
    machines: [
      { name: "lab", online: true, system: "Linux", device: false },
      { name: "laptop", online: true, system: "Windows", device: false },
      { name: "studio", online: false, system: "macOS", device: false },
    ],
    agents: [
      { resident: "codex-7f3a", machine: "lab", agent: "Codex", project: "mark50", title: "训练关节模型 400 轮", state: "working", since: new Date(Date.now() - 28 * 60_000).toISOString(), last: "tail -n 40 /data/mark50-sim/train.log" },
      { resident: "claude-2b91", machine: "laptop", agent: "Claude", project: "mark50", title: "整理调参记录", state: "idle", since: new Date(Date.now() - 4 * 60_000).toISOString(), last: "写了一条手记 tuning" },
    ],
  }),
  "file.read": (args) => {
    const text = String(args.path).endsWith(".csv")
      ? "run,lr,batch,val_loss,acc\n7,0.0003,64,0.412,0.884\n8,0.001,64,0.455,0.861\n9,0.00015,64,0.418,0.879\n10,0.0003,128,0.426,0.872\n"
      : String(args.path).endsWith(".jsonl")
        ? metrics.map((m) => JSON.stringify(m)).join("\n")
        : `${"…\n"}${log}\n`;
    return { text, size: text.length, modified: new Date().toISOString(), offset: args.tail ? 1 : 0 };
  },
  "job.read": () => ({ job: "j-k3x9a2", command: "python train.py --run 7 --steps 3000", state: "running", exit: null, signal: null, started: new Date(Date.now() - 51 * 60_000).toISOString(), output: log }),
  run: () => ({ exit: 0, output: "utilization.gpu [%]\n83 %\n79 %" }),
  say: () => ({ delivered: true }),
  open: () => ({}),
};
