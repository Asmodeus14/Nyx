use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight, RasterizedChar};

pub const CHAR_WIDTH: usize = 9;
pub const CHAR_HEIGHT: usize = 16;

pub fn get_char_raster(c: char) -> Option<RasterizedChar> {
    get_raster(c, FontWeight::Regular, RasterHeight::Size16)
}