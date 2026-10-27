# How to save designs for cutting

This guide helps you save a design so CutCraft cuts the letters exactly as you drew them.

## Why fonts matter

Your design may use a font that is not on the computer running CutCraft. Then CutCraft has
nothing to draw those letters with. It can only borrow another font. The borrowed font has
letters of different shapes and widths, so words can overlap, shift or run off their path.

The fix is easy: before you save the copy for cutting, turn the text into shapes. Shapes look
the same on every computer. Illustrator and VectorCraft call this **Create Outlines**.

## Before you save

1. Finish your design and save it as usual. This is your editable copy. Keep it.
2. Make a second copy for cutting. Give it a name that ends in "-outlined", for example
   "Summer shirt-outlined.ai".
3. In the copy for cutting, turn all text into shapes (see the steps below).
4. Open the copy for cutting in CutCraft.

Text that has become shapes can no longer be typed in. To change a word later, edit your
editable copy, then make a new copy for cutting.

Some people keep the editable text on a hidden, locked layer in the same file instead.
A separate copy is simpler and safer. If CutCraft still asks about a font, use a separate copy.

## Illustrator (Mac)

These steps match Illustrator 2023 to 2026. Menus can move a little between versions.

### Turn all text into shapes

1. Open your editable copy.
2. Choose **File > Save a Copy**. Name it "...-outlined" and choose the **Adobe Illustrator**
   format. In Illustrator Options, make sure **Create PDF Compatible File** is ticked, then
   click **OK**.
3. Open the "-outlined" copy.
4. If some text is locked, choose **Object > Unlock All**. Locked text is not selected in the
   next step. (Hidden things are not cut, so leave them hidden.)
5. Choose **Select > Deselect**. Then choose **Select > Object > Text Objects**. All your text
   is now selected.
6. Choose **Type > Create Outlines** (keyboard: Shift + Command + O).
7. Check that no text is left: choose **Select > Deselect**, then
   **Select > Object > Text Objects** again. If nothing is selected, all text is shapes.
   You can also open **Type > Find/Replace Font**: its list of fonts in the document should be
   empty.
8. Choose **File > Save**.

### Other ways to save the outlined copy

- **PDF:** choose **File > Save a Copy** and pick the **Adobe PDF** format, then click
  **Save PDF**. (**Save As** would switch the open document to the PDF.)
- **SVG:** choose **File > Export > Export As** and pick the **SVG** format. In SVG Options, set
  **Font** to **Convert To Outlines**. This turns the text into shapes in the SVG only. Your
  Illustrator file keeps its text.

### If you must keep the text as text

Shapes are best. If you cannot outline the text, CutCraft may still be able to cut the letter
shapes saved inside the file:

1. Save in the **Adobe Illustrator** format with **Create PDF Compatible File** ticked. Illustrator
   then saves the fonts inside the file, but it may not save every font.
2. Or save in the **Adobe PDF** format. Illustrator saves the letters you used inside the PDF.
   Leave the font settings under **Advanced** as they are.
3. Some fonts' licences do not allow saving them inside a file. Text in those fonts must be
   turned into shapes.

### Collect your fonts

**File > Package** copies the design, its pictures and its font files into one folder. Tick
**Copy Fonts** and **Create Report**. The report lists the fonts the design uses and any that
are missing. Check your font licence before copying fonts to another computer.

## VectorCraft

These steps match the VectorCraft version CutCraft is built with.

### Turn all text into shapes

1. Choose **File > Save a Copy** and save the "-outlined" copy as an Illustrator (.ai) file.
   **Create PDF-Compatible File** is on unless you turned it off. Then open that copy.
2. Choose **Select > Object > All Text Objects**.
3. Choose **Type > Create Outlines** (keyboard: Shift + Command + O). You can also right-click
   the text and choose **Create Outlines**.
4. Check: choose **Type > Find Font**. No fonts should be listed.
5. Choose **File > Save**.

### Save the outlined text straight into a PDF or SVG

You can skip Create Outlines and let the save do it. Your VectorCraft file keeps its text.

- **PDF:** choose **File > Save as PDF**. Under **Advanced**, in **Fonts**, tick
  **Convert text to outlines**.
- **SVG:** choose **File > Export > Export As SVG**. Set **Font** to **Convert To Outlines**.
- To make SVG exports use outlines every time, choose **File > Document Setup**. On the
  **Type** tab, set **Export** to **Preserve Text Appearance**.

### Collect your fonts

**File > Package** copies the design, its linked files and its fonts into one folder. Fonts
whose licence does not allow it are not copied.

## What to open in CutCraft

Best:
- An Illustrator (.ai), PDF or SVG file with all text turned into shapes.
- A flat, ASCII DXF exported from CAD model space, with the drawing units declared.
  CutCraft asks for units if they are missing; unfilled CAD paths cut on their centrelines.

OK:
- An Illustrator (.ai) file saved with **Create PDF Compatible File**, or a PDF, with the fonts
  saved inside. CutCraft may ask what to do about fonts (see below).
- A PNG, JPEG, GIF or BMP picture of clean, flat artwork. CutCraft traces it into cut lines.

Avoid:
- Silhouette Studio files (.studio3). CutCraft cannot open them. Open the original SVG, PDF or
  AI file instead.
- Screenshots, photos and mockups (for example a design shown on a shirt). Tracing them does not
  give clean cut lines. Use the original artwork.
- A file name with "outlines" in it is not proof. Check with **Select > Object > Text Objects**.

## CutCraft's font questions

When a font is not installed on the computer running CutCraft, CutCraft stops and asks. Your
text is left out of the preview until you answer.

**Use the letter shapes saved in the file.** The file carries the shapes of every font it uses.
CutCraft cuts those shapes, so the letters look as you designed them. Choose this whenever it is
offered. CutCraft offers it only when it can use the saved shapes for every font in the design.

**Use this substitute.** CutCraft names a font from this computer that would stand in. Its
letters have a different shape and width. Words may overlap, move or no longer fit their path.
Choose it only if the preview looks right to you, for example for plain text in a common font.

**Install the font and reopen.** This is not a button. Install the font on the computer running
CutCraft, then close the design and open it again. On a Mac, double-click the font file and
click **Install**. Choose this when you have the font file and are allowed to use it there.

The best answer of all: go back to your editor, turn the text into shapes, save, and open the
outlined copy. Then CutCraft has no font question.

### Other text questions

**Text that does not fit.** The installed font may differ a little from the one you designed
with, so some text no longer fits its frame or path.
- **Use the page appearance** keeps the text as it was designed.
- **Leave out the text that doesn't fit** cuts the rest of the design without it.

**Missing characters.** Some letters are not in the font, so CutCraft cannot cut them. Change the
text, use another font, or turn the text into shapes in your editor.

## Quick checklist

1. I kept an editable copy of my design.
2. In the copy for cutting, I chose Select > Object > Text Objects, then Type > Create Outlines.
3. Select > Object > Text Objects now selects nothing.
4. I saved it as .ai (Create PDF Compatible File ticked), PDF or SVG.
5. I opened the outlined copy in CutCraft, not a .studio3 file or a mockup picture.
6. If CutCraft asks about a font, I choose "Use the letter shapes saved in the file" when it is
   offered, or go back and outline the text.

## Sources

Illustrator (Adobe Help Center; read through the Internet Archive's copies, Aug to Oct 2026):
- Save artwork (Illustrator Options, Create PDF Compatible File, SVG Font Type):
  https://helpx.adobe.com/illustrator/using/saving-artwork.html
- Create Adobe PDF files (Save As compared with Export, Save Adobe PDF):
  https://helpx.adobe.com/illustrator/using/creating-pdf-files.html
- Adobe PDF options (subset embedded fonts):
  https://helpx.adobe.com/illustrator/using/pdf-options.html
- Export artwork (Export As, SVG export options, Font):
  https://helpx.adobe.com/illustrator/using/exporting-artwork.html
- Package files (Copy Fonts, Create Report):
  https://helpx.adobe.com/illustrator/using/package-files.html
- Select objects by characteristics (Select > Object > Text Objects):
  https://helpx.adobe.com/illustrator/desktop/manage-objects/select-objects/select-objects-by-characteristics.html
- Find and replace fonts (Type > Find/Replace Font):
  https://helpx.adobe.com/illustrator/desktop/design-with-text/fonts-and-scripts/find-and-replace-fonts.html
- Preview, add, or replace missing fonts:
  https://helpx.adobe.com/illustrator/desktop/design-with-text/fonts-and-scripts/preview-add-or-replace-missing-fonts.html
- Default keyboard shortcuts (Create outlines: Shift + Command + O):
  https://helpx.adobe.com/illustrator/using/default-keyboard-shortcuts.html
- Create outline text (Illustrator on the web; outlined text can no longer be edited as text):
  https://helpx.adobe.com/illustrator/web/add-and-edit-artwork/create-text/create-outline-text.html

Mac:
- Install fonts with Font Book: https://support.apple.com/guide/font-book/install-and-validate-fonts-fntbk1000/mac

VectorCraft: its command list and dialogs at the revision CutCraft pins (Type > Create Outlines,
Select > Object > All Text Objects, Type > Find Font, Save as PDF > Advanced > Fonts, Export As
SVG > Font, Document Setup > Type > Export, File > Package).

CutCraft: the font questions come from Make cut-ready (spec section 5.1).
