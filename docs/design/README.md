# Design

The harness's visual design: the logo, the icons, the themes and the reference mockups of the terminal UI.

## Logo

`logo/multitool.svg` is the logo: a fork, an orange handle with two eyes, a saw — traced from `concept/startup-light.png`, the owner's pick. The TUI recolours it per theme (tools in the text colour, the handle in the brand orange `#F5A623`, ink for the eyes and outline) and shows it as a picture in terminals with a graphics protocol.

In text mode the logo is pixel art in half blocks, 50 × 8 pixels in 4 rows (`LOGO_ART` in `crates/frontend-tui/src/images.rs`): fork and saw 11 px each and as tall as each other, the handle centred, eyes with a glint. On launch the eyes blink twice (`--eyes-closed` renders the closed frame). The one-row header carries no text-mode logo: one row is too small for the tool to read.

## Icons

`icons/*.svg`: 24 × 24 strokes in `#000000`, recoloured per use. Each has a text glyph for terminals without pictures (`Icon::fallback`).

## Themes

`../../themes/*.toml`: dark, light, paper, sand, blossom, mist (CON-180), nord, monokai, cyan — sixteen semantic roles, the brand colour, and the optional `accent_line` and `agent` roles.

## Mockups

[`concept/`](concept/README.md) holds the reference screens; [`../tui.md`](../tui.md) lists the named states that render them and the layout rules settled while matching them.
