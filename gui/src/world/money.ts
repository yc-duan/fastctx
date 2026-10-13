// Spend as the interface writes an estimate: dollars to the cent, thousands
// grouped, so the same amount reads the same on every page.

export function dollars(amount: number): string {
  return `$${amount.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

/** Whether an amount would show as $0.00 although something was spent. */
export function underCent(amount: number): boolean {
  return amount > 0 && amount < 0.005;
}
