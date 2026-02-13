// nyx-kernel/src/drivers/block.rs
pub trait BlockDevice {
    fn read_sector(&mut self, sector: u64, buf: &mut [u8]) -> bool;
    fn write_sector(&mut self, sector: u64, buf: &[u8]) -> bool;
}