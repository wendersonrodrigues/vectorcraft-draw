# vectorcraft-cdr

An independent, bounded Rust reader for CorelDRAW drawings (`.cdr`) saved as a ZIP package, as
recent CorelDRAW versions save them. It reads the package, the RIFF chunk tree in
`content/root.dat` with its references into the data streams, and the part of the document model
listed below, and hands the engine pages in points with every transform applied. It does not
write CorelDRAW files.

## Provenance (clean room)

Every layout here was worked out from the bytes of CorelDRAW files their owners saved and lent
for this, read locally and never committed, and from PKWARE's public ZIP APPNOTE. No other
reader's code (libcdr, UniConvertor, Inkscape) was consulted, and the CorelDRAW program itself
was never run, inspected or disassembled. Hypotheses were accepted only when the files confirmed
them: the decoded geometry of every rectangle, and of every curve up to control points outside
the curve, falls on the bounding box (`bbox`) the file records for the object.

Files examined so far: one two-page drawing saved by CorelDRAW 2022 (`AppVersion` 2410,
`CDRP` tree) with curves, rectangles, text, bitmaps and fountain fills. More versions are needed;
see "Not read yet".

## What is read

| CorelDRAW | VectorCraft |
|---|---|
| Package: stored `mimetype`, `content/root.dat`, `content/dataFileList.dat`, `content/data/*` | — |
| Pages (`LIST page`, the first is the master page), page size from `mcfg` | artboards, side by side |
| Layers (`LIST layr`, name in UTF-16); guide, desktop and grid layers are left out | layers (same name on several pages: one layer) |
| Groups (`LIST grp `) | groups |
| Curves (object type 3): nodes with move, line, cubic and close flags | paths |
| Rectangles (object type 1): width and height through the object's transform | paths (corner rounding not read yet) |
| Bitmaps (object type 5): a box in object space and the id of a `bmp ` record (`LIST bmpt`, data in `Bitmaps.dat`): 24-bit BGR rows bottom first, optional 8-bit mask | embedded PNG images with transparency, placed through the object's transform |
| Transforms (`trfd`: a, b, tx, c, d, ty) | applied to the geometry |
| PowerClip: a container's argument `0x1F45` names (first u32) the `spnd` of a group in the document's `LIST clpt`, whose objects it clips | clip group; the container's fill behind the contents, its outline over them |
| Style JSON: uniform fills, fountain fills (two colours, type 1 linear, type 2 radial), outline width and colour, uniform transparency; RGB255, CMYK and CMYK255 colours | fill and stroke appearance, opacity |

Bitmap records were checked the same way: the decoded image shows the expected picture upright
only when rows are read bottom first, its colours are right only as blue-green-red, its mask
cuts out the background, and the box through the object's transform lands on the object's
recorded bounding box to within one unit.

Radial fountain fills (type 2) put the second colour at the centre and the first at the edge, the
centre moved by `xCenterOffset`/`yCenterOffset` (percent of the object's size, y up), with a radius
of half the object's longer side. That reading was checked against the page preview CorelDRAW
saves in the package (`previews/page1.png`): rendered at the preview's size, the background
matches it within two levels per channel wherever no text or bitmap covers it.

Coordinates are 254 000 units to the inch (0.1 µm), y up, origin at the page centre. That unit is
an inference (it makes the sample's page 98 × 100 cm and the master page's desktop 1800 inches
tall); no file seen states it.

## Not read yet

Text (object type 4), bitmaps of other colour depths (only 24-bit colour with an 8-bit mask has
been seen), other object types (22, 31…),
rounded rectangle corners, ellipses and polygons as their own types, fountain fill intermediate
colours, steps, `xScale`/`yScale` and the linear fill's colour order (inferred: first colour at the
start), pattern and texture
fills, dashes, arrows, effects, and older files that are one RIFF file instead of a
package (refused with a message).

## Testing

`cargo test -p vectorcraft-cdr` runs on synthetic packages built by the `synth` module (a test
builder, not an exporter). Set `VECTORCRAFT_CDR_SAMPLES` to a folder of `.cdr` files you own to
also read those.
