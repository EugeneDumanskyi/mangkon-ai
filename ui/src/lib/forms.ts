// Pure helpers of the start screen and the settings form (US-01.1, US-01.6).

/** Which field a create error belongs to, so it shows next to it (US-01.1 AC2–AC3). */
export function createErrorField(code: string | null): "name" | "location" | null {
  switch (code) {
    case "name":
    case "name_taken":
      return "name";
    case "not_writable":
    case "folder_exists":
      return "location";
    default:
      return null;
  }
}

/** The domain stop list as typed: one per line or comma-separated, blanks dropped. */
export function parseDomains(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((d) => d.trim())
    .filter((d) => d !== "");
}

/** The OCR languages to offer: the installed ones, then any set but not installed. */
export function ocrChoices(installed: string[], chosen: string[]): string[] {
  return [...installed, ...chosen.filter((c) => !installed.includes(c))];
}
