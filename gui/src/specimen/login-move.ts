// A specimen server's Fastctx moving a login's directory, as
// `fastctx move-login` there would: its key shown the first time, what the
// command writes told line by line, then the move as its records say. A
// target under /full has no room, and is refused before anything moves.

import type { RemoteKey, RemoteServer } from "../app/remote-world";
import type { MoveAsk, Moved, MoveRecord } from "../world/login-move";

export interface SpecimenLoginMove {
  probe(host: string, port: number): Promise<RemoteKey>;
  move(server: RemoteServer, machine: string, hub: string, ask: MoveAsk, told: (piece: string) => void): Promise<Moved>;
  dirs(machine: string): Promise<string[]>;
}

export function specimenLoginMove(): SpecimenLoginMove {
  const records: MoveRecord[] = [];
  const after = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
  const say = async (told: (piece: string) => void, lines: string[]) => {
    for (const line of lines) {
      told(`${line}\n`);
      await after(350);
    }
  };
  const moved = (status: number | null, op: string | null): Moved => ({ status, records: records.map((one) => ({ ...one })), op });
  return {
    probe: async (host, port) => {
      await after(500);
      return { address: `203.0.113.30:${port}`, fingerprint: "SHA256:Vb7nq2XkR0c4Jm8sYt1LwPz9aE5uHd3gFo6iCk2NxQs", known: host === "known" ? "known" : "unknown" };
    },
    move: async (_server, _machine, _hub, ask, told) => {
      if (ask.do === "status") return moved(null, null);
      if (ask.do === "resume") {
        await say(told, ["no move is unfinished"]);
        return moved(0, null);
      }
      if (ask.do === "drop_backup") {
        const record = records.find((one) => one.op === ask.op);
        if (record) {
          await say(told, [`deleted ${record.backup}`]);
          record.backup_deleted = true;
        }
        return moved(0, ask.op);
      }
      const op = "20261122T031502-5f3a9c1e";
      const agent = ask.login.includes("codex") ? "codex" : "claude";
      if (ask.target.startsWith("/full")) {
        await say(told, [`fastctx: ${ask.target} has 1.2 GB free; the login takes 4.7 GB: nothing is done`]);
        return moved(1, null);
      }
      const record: MoveRecord = {
        op,
        agent,
        login: ask.login,
        source: ask.login,
        target: ask.target,
        backup: `${ask.login}.fastctx-moved-${op}`,
        phase: "stopping",
        service_restored: false,
        backup_deleted: false,
        why: null,
        at: new Date().toISOString(),
      };
      records.push(record);
      await say(told, [
        `fastctx-move: ${op}`,
        `moving ${ask.login} (${agent}) to ${ask.target}: the node is stopped until it is done (move ${op})`,
        ...(ask.stop_agents ? ["stopping claude (pid 48121), its 3 children with it"] : []),
        `copied 18 204 entries, 4.7 GB; checked against the original`,
      ]);
      record.phase = "done";
      record.service_restored = true;
      await say(told, [
        `moved: ${ask.login} -> ${ask.target} (move ${op})`,
        `the original is kept at ${record.backup} until \`fastctx move-login --drop-backup ${op}\``,
        "check before deleting it: the login keeps its id; old conversations go on (also /resume, --continue, --resume <id> in Claude Code); plugins and MCP servers; usage",
      ]);
      return moved(0, op);
    },
    dirs: async () => ["/root/.claude", "/root/.codex"],
  };
}
