#!/usr/bin/env python3
"""Generate apps/imageviewer/assets/sample.bmp — a 24-bit uncompressed BMP.

No third-party deps (Pillow not required): we hand-write the BITMAPFILEHEADER +
BITMAPINFOHEADER and bottom-up BGR rows, which is exactly what decode_bmp() in
main.rs consumes. The picture is a diagonal RGB gradient with a few solid bars so
that correct orientation, channel order, and scaling are all obvious at a glance.
"""
import struct

W, H = 480, 320


def pixel(x, y):
    # Diagonal gradient + orientation cues.
    r = int(255 * x / (W - 1))
    g = int(255 * y / (H - 1))
    b = int(255 * (1 - x / (W - 1)))
    # Solid channel bars across the top 40 px so B/G/R order is visually verifiable.
    if y < 40:
        third = x * 3 // W
        return [(255, 0, 0), (0, 255, 0), (0, 0, 255)][third]
    # A white border to confirm no off-by-one / row-padding drift.
    if x < 3 or x >= W - 3 or y < 3 or y >= H - 3:
        return (255, 255, 255)
    return (r, g, b)


def main():
    row_stride = (W * 3 + 3) & ~3
    pad = row_stride - W * 3
    pixel_data = bytearray()
    # BMP is bottom-up: emit the last visual row first.
    for y in range(H - 1, -1, -1):
        for x in range(W):
            r, g, b = pixel(x, y)
            pixel_data += bytes((b, g, r))  # BMP memory order is B, G, R
        pixel_data += b"\x00" * pad

    data_off = 14 + 40
    file_size = data_off + len(pixel_data)

    file_hdr = b"BM" + struct.pack("<IHHI", file_size, 0, 0, data_off)
    info_hdr = struct.pack(
        "<IiiHHIIiiII",
        40,            # biSize
        W, H,          # width, height (positive => bottom-up)
        1, 24,         # planes, bpp
        0,             # BI_RGB (uncompressed)
        len(pixel_data),
        2835, 2835,    # ~72 DPI
        0, 0,
    )

    with open("apps/imageviewer/assets/sample.bmp", "wb") as f:
        f.write(file_hdr)
        f.write(info_hdr)
        f.write(pixel_data)

    print(f"wrote apps/imageviewer/assets/sample.bmp ({file_size} bytes, {W}x{H} 24-bit)")


if __name__ == "__main__":
    main()
