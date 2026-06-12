use alloc::string::String;
use alloc::vec::Vec;
use crate::vfs::FileSystem;

pub struct TarFs {
    data: &'static [u8],
}

impl TarFs {
    pub fn new(data: &'static [u8]) -> Self {
        Self { data }
    }

    // TAR files store file sizes as octal strings (e.g., "0000123\0")
    fn parse_size(octal: &[u8]) -> usize {
        let mut size = 0;
        for &b in octal {
            if b >= b'0' && b <= b'7' {
                size = size * 8 + (b - b'0') as usize;
            } else if b == 0 || b == b' ' {
                break;
            }
        }
        size
    }

    // Scans the TAR headers to find the requested file
    fn find_file(&self, path: &str) -> Option<(usize, usize)> {
        let mut offset = 0;
        let target = path.trim_start_matches('/');
        
        while offset + 512 <= self.data.len() {
            let header = &self.data[offset..offset + 512];
            if header[0] == 0 { break; } // Two null blocks indicate the end of the archive
            
            let name_len = header.iter().take(100).position(|&c| c == 0).unwrap_or(100);
            let name = core::str::from_utf8(&header[..name_len]).unwrap_or("");
            
            let size = Self::parse_size(&header[124..136]);
            let data_offset = offset + 512;
            
            if name == target {
                return Some((data_offset, size));
            }
            
            // Jump to the next header (data blocks are padded to 512 bytes)
            offset += 512 + ((size + 511) / 512) * 512;
        }
        None
    }
}

impl FileSystem for TarFs {
    fn read_file(&self, path: &str, offset: usize, buf: &mut [u8]) -> Option<usize> {
        let (file_offset, file_size) = self.find_file(path)?;
        if offset >= file_size { return Some(0); }
        
        let read_size = core::cmp::min(buf.len(), file_size - offset);
        buf[..read_size].copy_from_slice(&self.data[file_offset + offset .. file_offset + offset + read_size]);
        Some(read_size)
    }

    fn get_file_size(&self, path: &str) -> Option<usize> {
        self.find_file(path).map(|(_, size)| size)
    }

    // TAR Initramfs is Read-Only!
    fn write_file(&mut self, _path: &str, _offset: usize, _buf: &[u8]) -> Option<usize> { None }
    fn create_file(&mut self, _path: &str) -> bool { false }
    fn create_dir(&mut self, _path: &str) -> bool { false }

    fn list_dir(&self, path: &str) -> Vec<String> {
        let mut results = Vec::new();
        let mut offset = 0;
        let target_dir = path.trim_start_matches('/');
        let target_prefix = if target_dir.is_empty() { String::new() } else { alloc::format!("{}/", target_dir) };

        while offset + 512 <= self.data.len() {
            let header = &self.data[offset..offset + 512];
            if header[0] == 0 { break; }
            
            let name_len = header.iter().take(100).position(|&c| c == 0).unwrap_or(100);
            let name = core::str::from_utf8(&header[..name_len]).unwrap_or("");
            let size = Self::parse_size(&header[124..136]);
            
            if name.starts_with(&target_prefix) {
                let remainder = &name[target_prefix.len()..];
                if !remainder.is_empty() && !remainder.contains('/') {
                    results.push(String::from(remainder));
                }
            }
            offset += 512 + ((size + 511) / 512) * 512;
        }
        results
    }
}