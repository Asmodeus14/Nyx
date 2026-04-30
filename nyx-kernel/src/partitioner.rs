use alloc::vec::Vec;
use core::cmp::Ordering;
use crate::drivers::nvme::NvmeDriver;

const FAT_SECTOR_SIZE: u64 = 512;
const NYXOS_REQUIRED_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2GB
const NYXOS_REQUIRED_SECTORS: u64 = NYXOS_REQUIRED_BYTES / FAT_SECTOR_SIZE;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct GptEntry {
    pub type_guid: [u8; 16],
    pub start_lba: u64,
    pub end_lba: u64,
}

impl PartialOrd for GptEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GptEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Sort partitions strictly by where they start on the disk
        self.start_lba.cmp(&other.start_lba)
    }
}

pub struct NyxPartitioner;

impl NyxPartitioner {
    /// Maps the drive and attempts to find a safe 2GB gap of unallocated space.
    pub fn find_free_space(driver: &mut NvmeDriver) -> Option<u64> {
        crate::serial_println!("[GPT] Initiating safe read-only drive mapping...");
        
        let mut entry_block = alloc::vec![0u8; 4096];
        let mut partitions = Vec::new();

        // 1. Read LBA 2 (The start of the Partition Entry Array)
        if !driver.read_block(2, &mut entry_block) {
            crate::serial_println!("[GPT] ERR: Failed to read partition array.");
            return None;
        }

        // 2. Parse all 32 potential partition slots
        for i in 0..32 {
            let offset = i * 128;
            if offset + 128 > entry_block.len() { break; }
            
            let mut type_guid = [0u8; 16];
            type_guid.copy_from_slice(&entry_block[offset..offset + 16]);
            
            // If the GUID is not all zeros, it is an active partition
            if type_guid.iter().any(|&b| b != 0) {
                let start_lba = u64::from_le_bytes(entry_block[offset + 32..offset + 40].try_into().unwrap());
                let end_lba = u64::from_le_bytes(entry_block[offset + 40..offset + 48].try_into().unwrap());
                
                partitions.push(GptEntry { type_guid, start_lba, end_lba });
            }
        }

        // 3. Sort partitions physically from front to back of the drive
        partitions.sort();

        crate::serial_println!("[GPT] Found {} active partitions. Scanning for {} sectors of free space...", partitions.len(), NYXOS_REQUIRED_SECTORS);

        // 4. Find the gaps between partitions
        // We start looking after LBA 34 (End of GPT reserved area)
        let mut current_search_lba: u64 = 34; 

        for part in &partitions {
            // Calculate the gap between our search cursor and the start of the next partition
            let gap_sectors = part.start_lba.saturating_sub(current_search_lba);
            
            if gap_sectors >= NYXOS_REQUIRED_SECTORS {
                crate::serial_println!("[GPT] SUCCESS: Found safe gap of {} sectors starting at LBA {}", gap_sectors, current_search_lba);
                return Some(current_search_lba);
            }
            
            // Move our search cursor to the end of this partition (+1)
            current_search_lba = part.end_lba + 1;
        }

        // 5. Check the very end of the drive (Gap between the last partition and the Backup GPT)
        // Note: For extreme safety, we assume a standard 1TB drive limits, but in a real scenario
        // you would read the total drive LBA count from the NVMe Identify Controller command.
        crate::serial_println!("[GPT] FAILED: No unallocated 2GB gap found between existing partitions.");
        None
    }
}