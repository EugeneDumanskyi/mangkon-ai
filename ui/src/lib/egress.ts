import type { Destination } from "../ipc/bindings/Destination";
import type { DestinationClass } from "../ipc/bindings/DestinationClass";
import type { KeyInput } from "../ipc/bindings/KeyInput";
import type { ProjectEndpoint } from "../ipc/bindings/ProjectEndpoint";
import type { Purpose } from "../ipc/bindings/Purpose";

/** US-09.7 AC4's names for where data goes. */
export function classLabel(c: DestinationClass): string {
  switch (c) {
    case "thisMachine":
      return "This machine";
    case "localNetwork":
      return "Local network";
    case "internet":
      return "Internet";
  }
}

/** What the data was sent for (US-09.8 AC2). */
export function purposeLabel(p: Purpose): string {
  switch (p) {
    case "extraction":
      return "AI extraction";
    case "embeddings":
      return "Embeddings";
    case "searchQuery":
      return "Search query";
  }
}

/** "llm.example.test:8080 · Local network · 192.168.1.20", with a warning for plain http. */
export function destinationLine(d: Destination): string {
  const parts = [`${d.host}:${String(d.port)}`, classLabel(d.class)];
  if (d.addresses.length > 0) parts.push(d.addresses.join(", "));
  if (!d.encrypted && d.class !== "thisMachine") parts.push("Not encrypted (http)");
  return parts.join(" · ");
}

/** An ISO 8601 UTC time, or milliseconds since the epoch, in the user's local time. */
export function shownTime(t: string | number): string {
  const d = new Date(t);
  return Number.isNaN(d.getTime()) ? String(t) : d.toLocaleString();
}

/** US-09.6 AC3's warning. */
export const NO_STRUCTURED_OUTPUT =
  "Structured output not supported — results may be less reliable";

/** What the form does with the key: a typed key replaces, "Remove" removes, else it stays. */
export function keyInput(typed: string, remove: boolean, editing: boolean): KeyInput {
  if (typed !== "") return { kind: "set", value: typed };
  if (remove || !editing) return { kind: "remove" };
  return { kind: "keep" };
}

/** The permission state in words (domain §7.6). */
export function accessLabel(e: ProjectEndpoint): string {
  const a = e.access;
  switch (a.kind) {
    case "thisMachine":
      return "This machine: no permission needed";
    case "notAllowed":
      return "Not allowed";
    case "allowed":
      return `Allowed for ${classLabel(a.class)} since ${shownTime(a.confirmedAt)}`;
    case "needsReconfirmation":
      return `Needs your confirmation again: ${a.reason}`;
  }
}
