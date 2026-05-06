use core::ptr::{read_volatile, write_volatile};
use spin::Mutex;

pub static INTEL_GPU: Mutex<Option<IntelGpuDriver>> = Mutex::new(None);
pub static mut BACKBUFFER_PHYS_ADDR: u64 = 0;

const BLT_RING_TAIL: u32 = 0x22030;
const BLT_RING_HEAD: u32 = 0x22034;
const BLT_RING_START: u32 = 0x22038;
const BLT_RING_CTL: u32 = 0x2203C;

const FORCEWAKE_BLT: u32 = 0xA188;
const FORCEWAKE_ACK_BLT: u32 = 0x130044;

// --- DISPLAY ENGINE REGISTERS ---
const PIPEA_STAT: u32 = 0x70024; // Pipe A Status (For V-Sync)

pub struct IntelGpuDriver {
    mmio_base: u64,
    device_id: u16,
    pub ring_virt_addr: Option<u64>,
}

impl IntelGpuDriver {
    pub fn new(mmio_base: u64, device_id: u16) -> Self {
        Self { mmio_base, device_id, ring_virt_addr: None }
    }

    pub unsafe fn read_reg(&self, offset: u32) -> u32 {
        read_volatile((self.mmio_base + offset as u64) as *const u32)
    }

    pub unsafe fn write_reg(&self, offset: u32, value: u32) {
        write_volatile((self.mmio_base + offset as u64) as *mut u32, value);
    }

    pub unsafe fn map_ggtt_page(&self, gpu_page_number: u32, phys_ram_addr: u64) {
        let gtt_offset = 0x200000 + (gpu_page_number as u64 * 8);
        let gtt_ptr = (self.mmio_base + gtt_offset) as *mut u64;
        let pte_value = (phys_ram_addr & 0xFFFFFFFFFFFFF000) | 0x03; 
        write_volatile(gtt_ptr, pte_value);
    }

    pub unsafe fn wake_up_gpu(&self) {
        self.write_reg(FORCEWAKE_BLT, 0x00010001);
        while (self.read_reg(FORCEWAKE_ACK_BLT) & 1) == 0 {
            core::hint::spin_loop(); 
        }
    }

    /// Bulletproof V-Sync: Waits for the monitor refresh, but times out safely
    /// if the monitor is plugged into a different hardware pipe (like Pipe B or C).
    pub unsafe fn wait_for_vsync(&self) {
        // Write-1-to-Clear the old V-Sync status flag
        self.write_reg(PIPEA_STAT, 1 << 1);
        
        let mut timeout = 0;
        // Spin until the monitor finishes its vertical refresh OR we hit the timeout
        while (self.read_reg(PIPEA_STAT) & (1 << 1)) == 0 && timeout < 150_000 {
            core::hint::spin_loop();
            timeout += 1;
        }
    }

    pub unsafe fn submit_command(&mut self, dwords: &[u32]) {
        if let Some(ring_ptr) = self.ring_virt_addr {
            let ring = ring_ptr as *mut u32;

            loop {
                let head_idx = self.read_reg(BLT_RING_HEAD) / 4;
                let tail_idx = self.read_reg(BLT_RING_TAIL) / 4;

                let free_space = if head_idx > tail_idx {
                    head_idx - tail_idx - 1
                } else {
                    1024 - tail_idx + head_idx - 1
                };

                if free_space >= dwords.len() as u32 { break; }
                core::hint::spin_loop();
            }

            let mut tail_idx = self.read_reg(BLT_RING_TAIL) / 4;
            for &dw in dwords {
                ring.add(tail_idx as usize).write_volatile(dw);
                tail_idx += 1;
                if tail_idx >= 1024 { tail_idx = 0; } 
            }
            self.write_reg(BLT_RING_TAIL, tail_idx * 4);
        }
    }

    pub unsafe fn wait_for_idle(&self) {
        let tail = self.read_reg(BLT_RING_TAIL) & 0x1FFFFC; 
        let mut head = self.read_reg(BLT_RING_HEAD) & 0x1FFFFC;
        let mut timeout = 0;
        while head != tail && timeout < 10000000 {
            head = self.read_reg(BLT_RING_HEAD) & 0x1FFFFC;
            core::hint::spin_loop();
            timeout += 1;
        }
    }

    pub unsafe fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32, pitch: u32) {
        let dest_gpu_addr = 0x900000; 
        let color_blit: [u32; 8] = [
            (0x2 << 29) | (0x50 << 22) | 0x05, 
            (3 << 24) | (0xF0 << 16) | pitch,  
            (y << 16) | x,                     
            ((y + h) << 16) | (x + w),         
            dest_gpu_addr,                     
            0x0,                               
            color,                             
            0x00000000                         
        ];
        self.submit_command(&color_blit);
        let flush_cmd: [u32; 4] = [(0x26 << 23) | 2, 0x0, 0x0, 0x0];
        self.submit_command(&flush_cmd);
    }

    pub unsafe fn copy_rect(&mut self, 
        src_x: u32, src_y: u32, src_pitch: u32, src_gpu_addr: u64,
        dst_x: u32, dst_y: u32, dst_pitch: u32, dst_gpu_addr: u64,
        w: u32, h: u32
    ) {
        let copy_blit: [u32; 10] = [
            (0x2 << 29) | (0x53 << 22) | 0x08, 
            (3 << 24) | (0xCC << 16) | dst_pitch, 
            (dst_y << 16) | dst_x,             
            ((dst_y + h) << 16) | (dst_x + w), 
            (dst_gpu_addr & 0xFFFFFFFF) as u32,
            (dst_gpu_addr >> 32) as u32,       
            (src_y << 16) | src_x,             
            src_pitch,                         
            (src_gpu_addr & 0xFFFFFFFF) as u32,
            (src_gpu_addr >> 32) as u32        
        ];
        self.submit_command(&copy_blit);
        let flush_cmd: [u32; 4] = [(0x26 << 23) | 2, 0x0, 0x0, 0x0];
        self.submit_command(&flush_cmd);
    }

    pub fn initialize(&mut self) {
        crate::serial_println!("[INTEL GPU] Initializing Gen9.5 Graphics Engine (Device ID: {:#06x})...", self.device_id);
        
        unsafe {
            // 1. RING BUFFER
            let ring_frame = crate::memory::allocate_frame().unwrap();
            let ring_phys = ring_frame.start_address().as_u64();
            self.map_ggtt_page(0, ring_phys); 
            self.ring_virt_addr = Some(crate::memory::map_mmio(ring_phys, 4096).unwrap());
            
            // 2. REAL SCREEN (0x100000)
            let fb_phys = crate::gui::FRAMEBUFFER_PHYS_ADDR;
            for i in 0..2048 {
                self.map_ggtt_page(256 + i as u32, fb_phys + (i as u64 * 4096)); 
            }

            // 3. SHARED GPU BACKBUFFER (0x900000)
            let mut bb_phys = 0;
            for i in 0..2048 {
                if let Some(frame) = crate::memory::allocate_frame() {
                    let phys = frame.start_address().as_u64();
                    if i == 0 { bb_phys = phys; }
                    self.map_ggtt_page(2304 + i as u32, phys);
                }
            }
            BACKBUFFER_PHYS_ADDR = bb_phys;

            // 4. WAKE UP & BOOT RING
            self.wake_up_gpu();
            self.write_reg(BLT_RING_CTL, 0);
            self.write_reg(BLT_RING_START, 0x0);
            self.write_reg(BLT_RING_HEAD, 0);
            self.write_reg(BLT_RING_TAIL, 0);
            self.write_reg(BLT_RING_CTL, 0x00000001);

            if (self.read_reg(BLT_RING_CTL) & 1) == 1 {
                crate::serial_println!("[INTEL GPU] -> SUCCESS: Engine Online!");
            }
        }
    }
}