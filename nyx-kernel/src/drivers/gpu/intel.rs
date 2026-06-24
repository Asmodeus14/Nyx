use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

pub static INTEL_GPU: Mutex<Option<IntelGpuDriver>> = Mutex::new(None);
// Phase 0: Replaced unsafe static mut with thread-safe AtomicU64
pub static BACKBUFFER_PHYS_ADDR: AtomicU64 = AtomicU64::new(0);

// --- PRM Verified Registers (Gen9/9.5 Command Stream Programming) ---
const BLT_RING_TAIL: u32 = 0x22030;
const BLT_RING_HEAD: u32 = 0x22034;
const BLT_RING_START: u32 = 0x22038;
const BLT_RING_CTL: u32 = 0x2203C;

const FORCEWAKE_BLT: u32 = 0xA188;
const FORCEWAKE_ACK_BLT: u32 = 0x130044;

// --- DISPLAY ENGINE REGISTERS ---
const PIPEA_STAT: u32 = 0x70024; // Pipe A Status (For V-Sync)

#[derive(Debug, PartialEq)]
pub enum GpuGeneration {
    Gen9,      // Broadwell/Skylake/Kaby Lake
    Gen11,     // Ice Lake
    Gen12,     // Tiger Lake/Xe
    Unsupported,
}

#[derive(Debug)]
pub enum GpuHangError {
    RingBufferFull,
    EngineHung,
}

pub struct IntelGpuDriver {
    mmio_base: u64,
    pub device_id: u16,
    pub generation: GpuGeneration,
    pub ring_virt_addr: Option<u64>,
}

impl IntelGpuDriver {
    pub fn probe(device_id: u16) -> GpuGeneration {
        match device_id {
            0x3185 | 0x3E9B | 0x5917 | 0x5916 => GpuGeneration::Gen9, 
            0x8A56 => GpuGeneration::Gen11,
            0x9A49 => GpuGeneration::Gen12,
            _ => GpuGeneration::Unsupported,
        }
    }

    pub fn new(mmio_base: u64, device_id: u16) -> Self {
        let generation = Self::probe(device_id);
        Self { mmio_base, device_id, generation, ring_virt_addr: None }
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

    pub unsafe fn wait_for_vsync(&self) {
        self.write_reg(PIPEA_STAT, 1 << 1);
        
        let mut timeout = 0;
        while (self.read_reg(PIPEA_STAT) & (1 << 1)) == 0 && timeout < 150_000 {
            core::hint::spin_loop();
            timeout += 1;
        }
    }

    // Phase 0: Added Ring Buffer Timeout/Watchdog
    pub unsafe fn submit_command(&mut self, dwords: &[u32]) -> Result<(), GpuHangError> {
        if let Some(ring_ptr) = self.ring_virt_addr {
            let ring = ring_ptr as *mut u32;
            let mut timeout = 0;

            loop {
                let head_idx = self.read_reg(BLT_RING_HEAD) / 4;
                let tail_idx = self.read_reg(BLT_RING_TAIL) / 4;

                let free_space = if head_idx > tail_idx {
                    head_idx - tail_idx - 1
                } else {
                    1024 - tail_idx + head_idx - 1
                };

                if free_space >= dwords.len() as u32 { break; }
                
                if timeout > 1_000_000 {
                    crate::serial_println!("[INTEL GPU] FATAL: Ring buffer full / GPU Hang detected!");
                    return Err(GpuHangError::RingBufferFull);
                }
                
                core::hint::spin_loop();
                timeout += 1;
            }

            let mut tail_idx = self.read_reg(BLT_RING_TAIL) / 4;
            for &dw in dwords {
                ring.add(tail_idx as usize).write_volatile(dw);
                tail_idx += 1;
                if tail_idx >= 1024 { tail_idx = 0; } 
            }
            self.write_reg(BLT_RING_TAIL, tail_idx * 4);
        }
        Ok(())
    }

    // Phase 0: Added Hang Detection
    pub unsafe fn wait_for_idle(&self) -> Result<(), GpuHangError> {
        let tail = self.read_reg(BLT_RING_TAIL) & 0x1FFFFC; 
        let mut head = self.read_reg(BLT_RING_HEAD) & 0x1FFFFC;
        let mut timeout = 0;
        
        while head != tail {
            if timeout > 10_000_000 {
                crate::serial_println!("[INTEL GPU] FATAL: Wait for idle timeout / GPU Hang!");
                return Err(GpuHangError::EngineHung);
            }
            head = self.read_reg(BLT_RING_HEAD) & 0x1FFFFC;
            core::hint::spin_loop();
            timeout += 1;
        }
        Ok(())
    }

    pub unsafe fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32, pitch: u32) -> Result<(), GpuHangError> {
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
        self.submit_command(&color_blit)?;
        let flush_cmd: [u32; 4] = [(0x26 << 23) | 2, 0x0, 0x0, 0x0];
        self.submit_command(&flush_cmd)?;
        Ok(())
    }

    pub unsafe fn copy_rect(&mut self, 
        src_x: u32, src_y: u32, src_pitch: u32, src_gpu_addr: u64,
        dst_x: u32, dst_y: u32, dst_pitch: u32, dst_gpu_addr: u64,
        w: u32, h: u32
    ) -> Result<(), GpuHangError> {
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
        self.submit_command(&copy_blit)?;
        let flush_cmd: [u32; 4] = [(0x26 << 23) | 2, 0x0, 0x0, 0x0];
        self.submit_command(&flush_cmd)?;
        Ok(())
    }

    pub fn initialize(&mut self) {
        crate::serial_println!("[INTEL GPU] Initializing {:?} Graphics Engine (Device ID: {:#06x})...", self.generation, self.device_id);
        
        unsafe {
            let ring_frame = crate::memory::allocate_frame().unwrap();
            let ring_phys = ring_frame.start_address().as_u64();
            self.map_ggtt_page(0, ring_phys); 
            self.ring_virt_addr = Some(crate::memory::map_mmio(ring_phys, 4096).unwrap());
            
            let fb_phys = crate::gui::FRAMEBUFFER_PHYS_ADDR;
            for i in 0..2048 {
                self.map_ggtt_page(256 + i as u32, fb_phys + (i as u64 * 4096)); 
            }

            let mut bb_phys = 0;
            for i in 0..2048 {
                if let Some(frame) = crate::memory::allocate_frame() {
                    let phys = frame.start_address().as_u64();
                    if i == 0 { bb_phys = phys; }
                    self.map_ggtt_page(2304 + i as u32, phys);
                }
            }
            // Safely store the backbuffer address globally
            BACKBUFFER_PHYS_ADDR.store(bb_phys, Ordering::SeqCst);

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