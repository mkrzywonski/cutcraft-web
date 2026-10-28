# Make cut-ready issue examples

These21 small original designs came from CutCraft 1.0.5 at
`39654a9465ecdef7478703ee586276304dbc29c6` (MIT OR Apache-2.0). The file bytes are unchanged.
The README has been corrected for the final1.0.5 behaviour rather than the older authoring date.
No file is from a private design archive. [Hashes and initial observations](../../docs/design-audit/fixtures.json)
record provenance; [the acceptance catalogue](../../docs/design-audit/acceptance.md) specifies the
full interaction/Undo/save/reopen checks. These are test inputs, not claims that the web UI is done.

Open/upload one at a time. Questions need an explicit answer; blockers need the stated recovery.
Show me identifies affected artwork where available. An accepted answer can expose another issue:
answering one question does not certify unrelated content. Resolved cards remain visible with a
tick, and Undo restores the prior decision.

| File | Expected issue or information |
|---|---|
| `01-no-physical-size.svg` | Confirm inferred **vector** physical size. |
| `02-missing-font.svg` | Missing-family substitution question under the controlled reference font set; upload the font or use the offered substitute explicitly. |
| `03-gradient-fill.svg` | Choose a solid colour or exclude the gradient shape. |
| `04-semi-transparent.svg` | Cut solid or exclude. |
| `05-embedded-picture.svg` | Exclude the embedded picture explicitly; retained vector artwork can be prepared. |
| `06-text-on-path-overflow.svg` | Text does not fit; explicitly accept only the fitting part or fix it in the editor. |
| `07-art-off-the-page.svg` | Use the artwork's area as the page. |
| `08-background-rectangle.svg` | Decide whether to cut the background rectangle; neither answer is automatic. |
| `09-tiny-pieces.svg` | Cut very small pieces anyway, or change the design. |
| `10-thin-pieces.svg` | Cut very thin pieces anyway, or change the design. |
| `11-near-duplicate-outlines.svg` | Keep both nearly repeating outlines, or fix the duplicate. |
| `12-dashed-line.svg` | Unsupported dashed **centreline** blocks; thick dashed shape conversion is different. |
| `13-empty.svg` | Nothing to cut; hidden art is informational. |
| `14-linked-picture.svg` | Missing/unverified linked image and import blockers, plus picture exclusion choice. Never read arbitrary server paths. |
| `15-outline-on-fill.svg` | Ready with stroke-as-part-of-shape Info; try Cut separately and Undo. |
| `16-two-pages.pdf` | Pick square or octagon page. |
| `17-logo-to-trace.png` | Conservative photo warning also appears for this antialiased logo in 1.0.5. Trace anyway remains available; size calculated automatically. |
| `18-photo.jpg` | Photo warning before tracing; use original artwork/editor for a clean design. Automatic size, not a size question. |
| `19-animated.gif` | Choose a composited frame; size is automatic. |
| `20-missing-letters.svg` | Missing glyphs under the reference font set; must not silently disappear. |
| `21-thick-line.svg` | Ready with thick-stroke-to-shape Info; red hairline remains a cut line. |

The pictures can also show low-resolution Info. Tracing may reveal additional small/thin-piece
questions. The exact-colour photo heuristic is a conservative warning, not proof of a photograph.
Font-dependent tests must establish their font prerequisites rather than rely on the host's fonts.

The [mountain badge](../manual/mountain-badge.svg) is a complete workflow/comparison example.
AI appearance-route, DXF, EPS, PSD, additional font and geometry regressions need the authored
fixtures/generators described in the acceptance catalogue; these21 examples do not cover them all.
