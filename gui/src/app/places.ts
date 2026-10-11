// The places the interface shows, and the address of each: what the hash
// in the window's address says, and what the sidebar marks as current.

import type { Address } from "../world/events";

/** The features a World can turn on (`world-gui.md`, 功能). */
export type Feature = "collaboration" | "memory";
const features: Feature[] = ["collaboration", "memory"];

export type Place =
  | { kind: "home" }
  | { kind: "talk"; address: Address }
  /** Beginning a conversation, in a project or outside any. */
  | { kind: "new"; town: string | null }
  | { kind: "project"; town: string }
  | { kind: "journal"; town: string; journal: string }
  /** A page of a project: a version of it, or the current one. */
  | { kind: "page"; town: string; page: string; version: number | null }
  | { kind: "machines" }
  | { kind: "machine"; name: string }
  /** The agents on every machine, or on one. */
  | { kind: "agents"; machine: string | null }
  /** The features, one card each, and a feature's own page. */
  | { kind: "features" }
  | { kind: "feature"; feature: Feature }
  /** Every conversation, or every one in a project: the ones the sidebar
   *  has no room for among them. */
  | { kind: "talks"; town: string | null }
  | { kind: "settings" }
  /** Logging in, the first time or to one more account. */
  | { kind: "login" }
  /** The accounts this computer or device is logged in to. */
  | { kind: "account" }
  /** An administrator's page of a hub's accounts. */
  | { kind: "admin" }
  /** A file on a machine, read whole, or its end when it keeps growing. */
  | { kind: "file"; machine: string; path: string; tail: boolean };

const part = encodeURIComponent;

export function link(place: Place): string {
  switch (place.kind) {
    case "home":
      return "#/";
    case "talk":
      return `#/talk/${part(`${place.address.resident}@${place.address.machine}`)}`;
    case "new":
      return place.town === null ? "#/new" : `#/new/${part(place.town)}`;
    case "project":
      return `#/project/${part(place.town)}`;
    case "journal":
      return `#/journal/${part(place.town)}/${part(place.journal)}`;
    case "page":
      return `#/page/${part(place.town)}/${part(place.page)}${place.version === null ? "" : `/${place.version}`}`;
    case "machines":
      return "#/machines";
    case "machine":
      return `#/machine/${part(place.name)}`;
    case "agents":
      return place.machine === null ? "#/agents" : `#/agents/${part(place.machine)}`;
    case "features":
      return "#/features";
    case "feature":
      return `#/feature/${place.feature}`;
    case "talks":
      return place.town === null ? "#/talks" : `#/talks/${part(place.town)}`;
    case "settings":
      return "#/settings";
    case "login":
      return "#/login";
    case "account":
      return "#/account";
    case "admin":
      return "#/admin";
    case "file":
      return `#/file/${part(place.machine)}/${place.tail ? "tail" : "whole"}/${part(place.path)}`;
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
    case "new":
      return { kind: "new", town: parts[1] || null };
    case "project":
      if (parts[1]) return { kind: "project", town: parts[1] };
      break;
    case "journal":
      if (parts[1] && parts[2]) return { kind: "journal", town: parts[1], journal: parts[2] };
      break;
    case "page":
      if (parts[1] && parts[2]) {
        const version = parts[3] ? Number(parts[3]) : null;
        return { kind: "page", town: parts[1], page: parts[2], version: version !== null && Number.isInteger(version) && version > 0 ? version : null };
      }
      break;
    case "machines":
      return { kind: "machines" };
    case "machine":
      if (parts[1]) return { kind: "machine", name: parts[1] };
      break;
    case "agents":
      return { kind: "agents", machine: parts[1] || null };
    case "features":
      return { kind: "features" };
    case "feature":
      if (features.includes(parts[1] as Feature)) return { kind: "feature", feature: parts[1] as Feature };
      return { kind: "features" };
    case "talks":
      return { kind: "talks", town: parts[1] || null };
    case "settings":
      return { kind: "settings" };
    // An address from before accounts, kept so old links still open.
    case "welcome":
    case "login":
      return { kind: "login" };
    case "account":
      return { kind: "account" };
    case "admin":
      return { kind: "admin" };
    case "file":
      if (parts[1] && parts[3]) return { kind: "file", machine: parts[1], path: parts.slice(3).join("/"), tail: parts[2] === "tail" };
      break;
  }
  return { kind: "home" };
}

export const talkLink = (address: Address) => link({ kind: "talk", address });

/** Where a file on a machine is read: whole, or its end when it grows. */
export const fileLink = (machine: string, path: string, tail = false) => link({ kind: "file", machine, path, tail });
