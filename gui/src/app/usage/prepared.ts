import type { HubAccount } from "../../world/accounts";
import type { UsageReport } from "../../world/usage";
import { usageOf, type Named, type Usage } from "./model";

/** One immutable report, prepared for its current reader. New reports and
 * changes to account names or ownership invalidate the derived totals. */
export class PreparedUsage {
  private previous: { report: UsageReport; names: string; usage: Usage } | null = null;

  get(report: UsageReport, nameOf: (account: HubAccount | null) => Named): Usage {
    // An undated legacy week uses the current time, so it is not reusable.
    if (!report.week.start || !report.week.end) return usageOf(report, nameOf);
    const names = JSON.stringify(report.people.map((person) => nameOf(person.account)));
    if (this.previous?.report === report && this.previous.names === names) return this.previous.usage;
    const usage = usageOf(report, nameOf);
    this.previous = { report, names, usage };
    return usage;
  }
}
