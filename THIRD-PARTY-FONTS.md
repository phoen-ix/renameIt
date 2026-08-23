# Fonts bundled with RenameIt

RenameIt shows filenames, and a filename is whatever the person who made it
typed. egui's own fonts cover Latin, Greek and Cyrillic; everything below is
bundled so a name in another script is legible rather than a row of `◻`.

The files live in `crates/ren-gui/assets/fonts/` and are compiled into the
executable with `include_bytes!` (`crates/ren-gui/src/theme.rs`). They are **not**
crates, so `cargo-deny` does not see them — which is why this file exists, and
why the release staging ships it alongside `LICENSE`.

All of them are licensed under the **SIL Open Font License, Version 1.1**,
whose full text is in `crates/ren-gui/assets/fonts/OFL.txt`. `deny.toml` already
allows `OFL-1.1`, and **D15** set the precedent for scoping a font licence.

| File | Upstream | Covers |
| --- | --- | --- |
| `NotoSansCJKjp-Regular.otf` | [notofonts/noto-cjk](https://github.com/notofonts/noto-cjk) `Sans/OTF/Japanese/` | Han, Hiragana, Katakana, Hangul |
| `NotoSansHebrew-Regular.ttf` | [notofonts/notofonts.github.io](https://github.com/notofonts/notofonts.github.io) `fonts/NotoSansHebrew/hinted/ttf/` | Hebrew |
| `NotoSansArabic-Regular.ttf` | same | Arabic, Persian, Urdu |
| `NotoSansThai-Regular.ttf` | same | Thai |
| `NotoSansDevanagari-Regular.ttf` | same | Hindi, Marathi, Nepali |

`NotoSansCJKjp` is the Japanese-preferred cut of the pan-CJK face: one file
covers Han, kana **and** Hangul, differing from the SC/TC/KR cuts only in which
regional Han glyph shapes it prefers. Coverage, not typographic locale, is what
a rename tool needs, so one face is bundled rather than four.

## Checksums

Verified at the commit that added them:

```
68a3fc98800b2a27b371f2fb79991daf3633bd89309d4ffaa6946fd587f375b5  NotoSansCJKjp-Regular.otf
bdff3e5659d67e67def05b33f749683b9376ae819d65d3dd62ac4640b3aaef48  NotoSansArabic-Regular.ttf
306b53ecfb182a504dd8a7446093c316387d2fd8dc350d0792ed1753fe0996cd  NotoSansDevanagari-Regular.ttf
cdefaf8efd47045f6820928eba84db5bed7557539328952b5f828315485e02ee  NotoSansHebrew-Regular.ttf
61cf814eec46b294d6ea4401ac295d0cecd5207bd2331dcc5a15e7301d30ee44  NotoSansThai-Regular.ttf
```

## What these fonts do not fix

Two limits are in the text engine rather than in the fonts, so no face changes
them:

- **Right-to-left word order.** egui has no bidi pass
  (`epaint/src/text/font.rs`: `// TODO(emilk): heed bidi characters`). A single
  Hebrew or Arabic word renders correctly — the shaper reverses it — but once a
  space, digit or Latin character intervenes the *words* come out in logical
  rather than visual order. Letterforms and Arabic joining are correct either
  way.
- **Colour emoji.** epaint's rasteriser is outline-only, with no COLR/CBDT/sbix
  path, so emoji render in the bundled monochrome face.

Neither stops a file being renamed: both are display, and the engine works on
the bytes.
