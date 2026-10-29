# Add OCR languages

> Input: ADR-0018 (OCR language files), ADR-0014 (Tesseract). **If this file disagrees with them, they win.**
> Status: Written in M3b (2026-10-29). Linux checked on RM-1. macOS not verified here; Windows untested.

Mangkon AI reads scans and scanned PDF pages with Tesseract. English is built in. For any other language you put one file into the app's language folder. The app never downloads language files.

---

## 1. Where the files go

One folder for all projects:

| OS | Folder |
|---|---|
| Linux | `$XDG_DATA_HOME/mangkon-ai/tessdata/` (usually `~/.local/share/mangkon-ai/tessdata/`) |
| macOS | `~/Library/Application Support/mangkon-ai/tessdata/` |
| Windows | `%APPDATA%\mangkon-ai\tessdata\` |

The app creates the folder and writes `eng.traineddata` there the first time it reads a PDF, an image or an email. Do not edit that file: the app writes it again if it differs from the built-in one.

`mangkon-cli` uses the same folder. `--tessdata DIR` points it elsewhere.

## 2. Get a language file

Each language is one file named `<code>.traineddata`, for example `deu.traineddata` (German) or `chi_sim.traineddata` (Simplified Chinese). Tesseract's own repositories have them:

- [`tessdata_fast`](https://github.com/tesseract-ocr/tessdata_fast): the same kind as the built-in English. Fast; a good default.
- [`tessdata_best`](https://github.com/tesseract-ocr/tessdata_best): slower, more accurate on poor scans.

Pick the file for your language and copy it into the folder from §1. Codes are letters, digits and `_` only; a file with another name is ignored.

The app does not check these files. Get them from a source you trust.

## 3. Use it in a project

A project lists its OCR languages, joined with `+`. The default is `eng`. OCR uses only the project's languages.

```sh
mangkon-cli ingest --project ~/cases/harbour --ocr-lang eng+deu ~/cases/harbour-files
```

The setting is kept in the project. The desktop app gets a settings screen for it in M3c.

If a language of the project has no file in the folder, each file that needs OCR fails with:

```text
OCR language not installed: deu
```

Text-native PDFs and the other formats are still read. A damaged language file fails the same way.

Files already read keep their text when the languages change. Reading them again after a change comes with the settings screen (M3c).
