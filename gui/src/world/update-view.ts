// The machines page's update view, as an account's hub tells it
// (`world-upgrade.md`, 谁来更新): asked through this device's node, the hub
// answers from what it keeps in memory, and schedules updates or sets the
// network's policy for whoever may.

import { invoke } from "@tauri-apps/api/core";
import type { UpdatesViewCall, UpdatesViewReply } from "./updates";

export function networkUpdates(hub: string, account: string, request: UpdatesViewCall): Promise<UpdatesViewReply> {
  return invoke<UpdatesViewReply>("network_updates", { hub, account, request });
}
