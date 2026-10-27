// Sizes as a person reads them, wherever a size is shown.

/** Bytes as a person reads them. */
export function bytesSaid(bytes: number): string {
  // A number and its unit are never parted at the end of a line.
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}
