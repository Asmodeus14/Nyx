# Atlas sizes

Seven shelves, not seven atlases. A shelf is a horizontal band of the same 512-wide texture, so adding a size costs vertical space and nothing else — no second binding, no second draw call, no second texture upload.

## Where the budget goes

The two largest sizes are used almost entirely for digits: the System Monitor's four readouts, the OSD value, workspace numerals. Packing the **full ASCII range** at 40px would spend more of the atlas on glyphs that never appear than the other five shelves use in total, so those two pack **0–9** and a handful of letters.

Coverage is one byte per pixel. At 512 wide the seven shelves come to roughly 180 KB, which is less than the embedded font file it was rasterised from.
