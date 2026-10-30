import { createContext } from "react";

/** What a link click in document text does: ask to open `url` (US-08.4 AC3). `null` shows links
 * as plain text. */
export const LinkContext = createContext<((url: string) => void) | null>(null);
