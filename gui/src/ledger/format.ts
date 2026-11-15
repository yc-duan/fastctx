// Figures as a ledger writes them: short, and the same width in a column.

export function tokens(count: number): string {
  if (count < 1000) return String(count);
  if (count < 1e6) return `${(count / 1000).toFixed(count < 1e4 ? 1 : 0)}k`;
  if (count < 1e9) return `${(count / 1e6).toFixed(count < 1e7 ? 2 : 1)}M`;
  return `${(count / 1e9).toFixed(2)}B`;
}

export function dollars(amount: number): string {
  if (amount >= 100) return `$${Math.round(amount).toLocaleString("en-US")}`;
  if (amount > 0 && amount < 0.001) return "< $0.001";
  if (amount > 0 && amount < 0.01) return `$${amount.toFixed(3)}`;
  return `$${amount.toFixed(2)}`;
}

export function seconds(amount: number): string {
  if (amount >= 60) return `${Math.floor(amount / 60)}m ${String(Math.round(amount % 60)).padStart(2, "0")}s`;
  return `${amount.toFixed(amount < 10 ? 1 : 0)}s`;
}

const two = (value: number) => String(value).padStart(2, "0");
export function day(at: Date): string {
  return `${two(at.getMonth() + 1)}-${two(at.getDate())}`;
}
export function clock(at: Date): string {
  return `${two(at.getHours())}:${two(at.getMinutes())}:${two(at.getSeconds())}`;
}

const weekdays = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
/** A day as a person says it: the month, the day, the weekday. */
export function dated(at: Date, weekday = true): string {
  return `${at.getMonth() + 1} 月 ${at.getDate()} 日${weekday ? `　${weekdays[at.getDay()]}` : ""}`;
}
/** Dollars to the whole dollar, as a quota bar says them. */
export function whole(amount: number): string {
  return `$${Math.round(amount).toLocaleString("en-US")}`;
}
export function count(amount: number): string {
  return Math.round(amount).toLocaleString("en-US");
}
