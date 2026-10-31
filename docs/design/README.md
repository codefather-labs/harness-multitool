# Design

## Logo

`logo/` holds the multitool logo as four SVG frames of one animation, all with the same handle and the same pivot (66,190):

| Frame | Saw | Fork |
|---|---|---|
| `multitool-1-closed.svg` | folded | folded |
| `multitool-2-saw-rising.svg` | 35° | folded |
| `multitool-3-saw-open-fork-rising.svg` | 60° | 50° |
| `multitool-4-open.svg` | 60° | 108° |

The handle lies horizontal, the tools unfold upward from the pivot at its left end, fanning out from left to right; only two tools, a saw and a fork. Outlines use `currentColor`, so the same file works on light and dark backgrounds; fills are three colours (orange handle `#F5A623`, highlight `#FFE0A0`, tools `#F2F2ED`). The tools are drawn behind the handle, so a folded tool is hidden inside it.

The style came out of a design session with ChatGPT; the geometry of the saw and the fork was redrawn here.

To do: the terminal version for the start screen (block characters, a few frames of the same unfolding), and the TUI screens.
