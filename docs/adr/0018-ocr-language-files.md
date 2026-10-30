# ADR-0018: OCR language files — English built in, others installed by the user

- **Status:** Accepted
- **Date:** 2026-10-29
- **Related:** INV-WS-06, US-01.6, US-02.2 AC2/AC4, NFR-SEC-02, NFR-SEC-11, NFR-LEG-05, NFR-RES-07, NFR-USE-01, requirements Q9, architecture §6, §10 gap A1, roadmap §4 #3, ADR-0014 (its M3 follow-up on language files), ADR-0015 (prerequisites over downloads), M3b, M3c

## Context

- Tesseract reads one `<code>.traineddata` file per language (ADR-0014). English, `tessdata_fast` 4.1.0 (4.1 MB), is pinned by SHA-256 in the build.
- A project always has at least one OCR language (INV-WS-06). The default is English (US-01.6). OCR uses only the project's languages (US-02.2 AC4).
- Requirements Q9: "English by default; user can add … language packs if the engine supports it."
- The app talks to the network only for model downloads (Hugging Face) and user-set endpoints (NFR-SEC-02). It downloads no binaries (gap A1, ADR-0015).
- Downloaded files are checked by SHA-256 before use (NFR-SEC-11).
- Tesseract publishes 125 languages and 37 scripts, one file each (ADR-0014). English alone is 4.1 MB; all of them together are hundreds of MB. ASSUMPTION: not measured.
- The app's size is capped at 150 MB without models (NFR-RES-07).
- Gap A1 left "where extra OCR language files come from" to M3 (architecture §10, roadmap §4 #3).
- The maintainer decided on 2026-10-29: other languages are user-installed `.traineddata` files in the app's `tessdata/` folder; no download.

## Options considered

**1. English built in, others installed by the user into the app's `tessdata/` folder.**
- ➕ No new network host, no download code, nothing to pin for files we do not ship (NFR-SEC-02/11).
- ➕ The app stays small (NFR-RES-07).
- ➕ Any Tesseract language works, including `tessdata_best` files or custom ones.
- ➖ The user fetches the file by hand. A wrong or damaged file is their file; the app cannot check its hash.

**2. Download on demand from GitHub (`tessdata_fast`), pinned by SHA-256 in a catalog.**
- ➕ One click in settings.
- ➖ A new download host beside Hugging Face (NFR-SEC-02 changes). Gap A1 decided against host sprawl.
- ➖ 125+ hashes to pin and keep current.

**3. Ship every language in the app.**
- ➕ Works offline at once.
- ➖ Hundreds of MB. Breaks NFR-RES-07.

**4. Mirror the language files on Hugging Face** (the model host).
- ➕ No new host.
- ➖ A mirror to maintain and keep in sync. It still needs a catalog of hashes.

## Decision

Use **option 1**.

**Folder**
- OCR language files live in `<app data>/tessdata/`, shared by all projects (architecture §6). `<app data>` is `$XDG_DATA_HOME/mangkon-ai` (default `~/.local/share/mangkon-ai`) on Linux, `~/Library/Application Support/mangkon-ai` on macOS, and `%APPDATA%\mangkon-ai` on Windows (`mk_core::app_data_dir`).
- The desktop app and `mangkon-cli` set this folder on the project they open. `mangkon-cli --tessdata DIR` overrides it.

**English**
- `eng.traineddata` (`tessdata_fast` 4.1.0) is embedded in the binary (`mk_tesseract_sys::ENG_TRAINEDDATA`). The build checks it against the SHA-256 pinned in `native/sources.toml`; a mismatch fails the build.
- Before OCR settings go to a worker, the core writes the embedded file into `tessdata/` when it is missing or differs from the embedded bytes. It writes next to the final name and renames into place.

**Other languages**
- The user copies `<code>.traineddata` files into `tessdata/`. Install note: `docs/install-ocr-languages.md`.
- The app never downloads a language file and never checks the hash of a user-installed one.
- A language code is 1–32 ASCII letters, digits or `_` (`deu`, `chi_sim`, `deu_latf`). Anything else is refused, so a code cannot reach outside the folder.
- `Project::installed_ocr_languages` lists the codes with a file in `tessdata/`.

**Project setting**
- The project keeps `ocr_languages` in `project_meta`: `+`-joined codes (`eng+deu`), default `eng` (US-01.6, INV-WS-06). An empty list or an invalid code is refused with a plain message.
- The core passes the languages and the folder to each worker. A worker loads one `TessBaseAPI` and loads it again only when the languages change (ADR-0014).
- A language with no file fails only the files that need OCR, with "OCR language not installed: deu" (NFR-USE-05). Text-native PDFs and other formats are read as usual.
- M3b sets the languages with `mangkon-cli ingest --ocr-lang eng+deu`. The settings screen comes in M3c (US-01.6).
- Changing the languages does not re-OCR files already read. ASSUMPTION: a re-read on language change comes with the M3c settings screen.

## Consequences

**Easier**
- No new network host. NFR-SEC-02 and gap A1 stay as decided.
- The app grows only by English (4.1 MB, NFR-RES-07).
- Any Tesseract language or model variant works, including `tessdata_best` for poor scans.

**Harder**
- Adding a language is a manual step outside the app (NFR-USE-01 counts English only).
- A user-installed file is not checked. Tesseract cannot load a damaged file, so it fails the files that need OCR as a missing one does: "OCR language not installed: deu".
- The folder is shared: removing a file affects every project that uses that language.

**Follow-ups**
- Architecture §6: add `tessdata/` to the app data layout (M3b).
- Architecture §10 gap A1 and roadmap §4 #3: record this decision (M3b).
- ADR-0014: point its M3 follow-up on language files here (M3b).
- M3c: the settings screen lists installed languages, picks the project's, and offers to re-read scans after a change (US-01.6).
- Tests (M3b): `us_02_2_ac4_ocr_languages` (missing language, invalid codes, English built in), `us_02_2_ac4_ocr_languages_setting` (store default and update).
