// The World program each machine of the specimen runs, so that its page, its
// readings and its updates tell one story: this device a test release, the
// server and the studio earlier ones, the GPU box one too old to say how it
// is doing.

export const SPECIMEN_VERSION = "1.0.1-beta.3";

const RUNNING: Record<string, string> = {
  server: "1.0.1-beta.2",
  studio: "1.0.1-beta.1",
  gpu: "0.9.0",
};

export function runningOn(machine: string): string {
  return RUNNING[machine] ?? SPECIMEN_VERSION;
}
