# Test fixtures: fonts

`LiberationMono-Regular.ttf` is a **test fixture**, not a shipped asset. It is
vendored unmodified so `ferrocad_text`'s tests (and the `Part.makeWireString`
acceptance test) are hermetic and run offline.

- Source: Liberation Fonts, <https://github.com/liberationfonts> (as packaged by
  Debian in `fonts-liberation`, `/usr/share/fonts/truetype/liberation/`).
- SHA-256: `d18235f1e874771f4eaec3ddd6ddb4db849d334bdc6930c5c2185d9344834e62`
- Size: 319 624 bytes.
- Copyright: Digitized data copyright (c) 2010 Google Corporation with Reserved
  Font Arimo, Tinos and Cousine. Copyright (c) 2012 Red Hat, Inc. with Reserved
  Font Name Liberation.
- Licence: SIL Open Font License 1.1 — see [`OFL.txt`](OFL.txt).

The font is **unmodified**, which is why it keeps its Reserved Font Name. If it
is ever subset (to shrink it), the OFL's Modified-Version clause requires renaming
it away from "Liberation".

Why a monospace font: a constant advance makes the layout assertions in
`src/outline.rs` (origins at `i × advance`, tracking shifts the next glyph) test
the layout rather than the font's per-glyph widths.
