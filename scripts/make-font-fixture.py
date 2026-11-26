#!/usr/bin/env python3
"""Regenerate the original tiny test font (requires fontTools, not needed to run tests).
Only an authored square glyph is included; no existing font or glyph is copied.
"""
from pathlib import Path
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

font = FontBuilder(1000, isTTF=True)
font.setupGlyphOrder(['.notdef', 'space', 'A'])
font.setupCharacterMap({32: 'space', 65: 'A'})
glyphs = {}
for name in ['.notdef', 'space', 'A']:
    pen = TTGlyphPen(None)
    if name != 'space':
        pen.moveTo((100, 0))
        pen.lineTo((600, 0))
        pen.lineTo((600, 700))
        pen.lineTo((100, 700))
        pen.closePath()
    glyphs[name] = pen.glyph()
font.setupGlyf(glyphs)
font.setupHorizontalMetrics({name: (700, 100) for name in glyphs})
font.setupHorizontalHeader(ascent=800, descent=-200)
font.setupNameTable({'familyName': 'CutCraft Fixture', 'styleName': 'Regular',
                    'uniqueFontIdentifier': 'CutCraftFixture-Regular-1',
                    'fullName': 'CutCraft Fixture Regular',
                    'psName': 'CutCraftFixture-Regular', 'version': 'Version 1.0'})
font.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
font.setupPost()
font.setupMaxp()
font.font['head'].created = font.font['head'].modified = 3700000000
font.font.recalcTimestamp = False
font.save(Path(__file__).resolve().parent.parent / 'crates/import/tests/fixtures/cutcraft-test-font.ttf')
