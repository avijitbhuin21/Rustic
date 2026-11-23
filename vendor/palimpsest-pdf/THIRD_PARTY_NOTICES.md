# Third-party notices

Palimpsest is licensed `MIT OR Apache-2.0`. This file records material that was
taken from other projects, as `CLEANROOM.md` requires. Crates that are only
linked as dependencies carry their own licences and are checked by
`cargo deny check licenses`; they are not listed here.

## Standard 14 font metrics

**File:** `crates/palimpsest-pdf/src/std14.rs`

**What:** advance widths of the standard 14 PDF fonts (Helvetica, Times,
Courier, Symbol, ZapfDingbats) and the code-to-Unicode tables of Symbol and
ZapfDingbats. Used when a font dictionary has no `Widths` array.

**Taken from:** the generated tables `core_fonts.rs`, `glyphnames.rs` and
`zapfglyphnames.rs` of [`pdf-extract`](https://github.com/jrmuizel/pdf-extract)
0.12.1, MIT licence, Copyright (c) Jeff Muizelaar. The tables were regrouped by
Unicode code point and reduced to six width columns; no code was copied.

`pdf-extract` in turn generated its width table from Adobe's Core 14 AFM files,
which are distributed under this notice:

> Copyright (c) 1985, 1987, 1989, 1990, 1991, 1992, 1993, 1997 Adobe Systems
> Incorporated. All Rights Reserved.
>
> This file and the 14 PostScript(R) AFM files it accompanies may be used,
> copied, and distributed for any purpose and without charge, with or without
> modification, provided that all copyright notices are retained; that the AFM
> files are not distributed without this file; and that any modifications made
> to the AFM files are prominently noted in the modified file(s).

Modification notice: the AFM data is not distributed here as AFM files. Only
the advance widths were kept, converted to a Rust table.

## Techniques

No source code of MuPDF, PyMuPDF, mupdf.js, Ghostscript, Poppler, Xpdf or iText
was read. The approach to text editing (read the run, remove its glyphs, typeset
the new text in the same place) follows the public documentation of existing
editors, as described in `ROADMAP.md`. File formats were implemented from
ISO 32000 (PDF) and the OpenType specification (`glyf`, `loca`, `hmtx`, sfnt
checksums).

## Development-only tools

`tools/verify/` renders pages for before/after comparison with `pdfjs-dist`
(Apache-2.0), `@hyzyla/pdfium` (MIT, bundling PDFium, BSD-3-Clause/Apache-2.0)
and `@napi-rs/canvas` (MIT), and with macOS PDFKit through `quartz.swift`.
None of this is linked into the engine or the CLI.
