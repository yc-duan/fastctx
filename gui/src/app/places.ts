// The places the interface shows, and the address of each: what the hash
// in the window's address says, and what the sidebar marks as current.

import type { Address } from "../world/events";

export type Place =
  | { kind: "home" }
  | { kind: "talk"; address: Address }
  | { kind: "project"; town: string }
  | { kind: "journal"; town: string; journal: string }
  | { kind: "machines" }
  | { kind: "machine"; name: string }
  | { kind: "settings" };

const part = encodeURIComponent;

export function link(place: Place): string {
  switch (place.kind) {
    case "home":
      return "#/";
    case "talk":
      return `#/talk/${part(`${place.address.resident}@${place.address.machine}`)}`;
    case "project":
      return `#/project/${part(place.town)}`;
    case "journal":
      return `#/journal/${part(place.town)}/${part(place.journal)}`;
    case "machines":
      return "#/machines";
    case "machine":
      return `#/machine/${part(place.name)}`;
    case "settings":
      return "#/settings";
  }
}

/** The place an address names; anything it does not name is the World. */
export function placeOf(hash: string): Place {
  const parts = hash.replace(/^#\/?/, "").split("/").map(decodeURIComponent);
  switch (parts[0]) {
    case "talk": {
      const name = parts.slice(1).join("/");
      const at = name.lastIndexOf("@");
      if (at > 0) return { kind: "talk", address: { resident: name.slice(0, at), machine: name.slice(at + 1) } };
      break;
    }
    case "project":
      if (parts[1]) return { kind: "project", town: parts[1] };
      break;
    case "journal":
      if (parts[1] && parts[2]) return { kind: "journal", town: parts[1], journal: parts[2] };
      break;
    case "machines":
      return { kind: "machines" };
    case "machine":
      if (parts[1]) return { kind: "machine", name: parts[1] };
      break;
    case "settings":
      return { kind: "settings" };
  }
  return { kind: "home" };
}

export const talkLink = (address: Address) => link({ kind: "talk", address });
