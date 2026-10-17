# Bundled typeface: Atkinson Hyperlegible Next 2.001

Used only by the Eskiz look (Actions > Settings). The Classic look keeps egui's default fonts.

- Source: https://github.com/googlefonts/atkinson-hyperlegible-next, `fonts/ttf/`, file `AtkinsonHyperlegibleNext-{Regular,SemiBold}.ttf` (font version string "Version 2.001; ttfautohint (v1.8.4.7-5d5b)"), and `OFL.txt` from the same repository (branch `main`, fetched 2026-10-17; the repository tags were not readable from the build environment, so the files are pinned by SHA-256 in `SHA256SUMS`).
- The variable font in that repository and in google/fonts has the identical SHA-256 (5a455d1c...), which confirms the same release.
- Licence: SIL Open Font License 1.1, copyright 2020-2024 The Atkinson Hyperlegible Next Project Authors. The copyright line carries no Reserved Font Name clause, and the files are embedded unmodified, so the licence is satisfied by shipping the licence text (in THIRD_PARTY_NOTICES.md, generated from about.hbs).
- The font has the OpenType `tnum` feature, but egui 0.36 cannot switch features on, and its default digits are proportional; slider values use fixed digit cells (`fonts::numeric`).
