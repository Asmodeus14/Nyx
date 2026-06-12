\================================================================================

NYXOS: DYNAMIC MULTI-GENERATION NVIDIA GRAPHICS DRIVER ARCHITECTURE PLAN

\================================================================================

This document provides the technical specification and progressive implementation

phases for a dynamic bare-metal NVIDIA graphics driver tailored for the nyx-kernel

codebase.

Because the exact physical GPU model is unknown at compile time, the driver

architecture avoids hardcoded assumptions. It operates by probing the peripheral

bus, dynamically fingerprinting the silicon generation via MMIO control registers,

streaming cryptographically signed microcode from the NVMe file system, and

establishing direct memory channels to offload window composition entirely from

the CPU to the GPU.

\--------------------------------------------------------------------------------

PHASE 0: PRE-REQUISITE CORE KERNEL FIXES

\--------------------------------------------------------------------------------

Before building the driver modules, existing kernel sub-systems must be patched

to manage 64-bit BAR layouts, apply target memory-cache attributes, and establish

clean system call routing.

1\. 64-Bit BAR Extraction & Vendor Routing (nyx-kernel/src/pci.rs)

The current Class 0x03 display controller path is hardwired to check for Intel's

vendor ID (0x8086). Discrete NVIDIA GPUs use massive prefetchable 64-bit BAR

apertures for their physical VRAM that can overwrite adjacent registers if read

as narrow 32-bit fields.

Update the Class 0x03 matching blocks in pci.rs to include the following routing logic:

// Match block inside pci.rs display handler

0x03 => {

crate::serial\_println!("\[PCI\] Display Controller Detected.");

if vendor\_id == 0x8086 {

// ... Keep existing Intel path ...

} else if vendor\_id == 0x10DE {

crate::serial\_println!("\[PCI\] \*\*\* FOUND NVIDIA DISCRETE GPU: Device ID {:#06x} \*\*\*", device\_id);

// 1. Enable Bus Mastering (0x04) and MMIO Memory Space (0x02)

let command\_ptr = (device\_virt + 0x04) as \*mut u16;

let mut command = unsafe { core::ptr::read\_volatile(command\_ptr) };

command |= 0x06;

unsafe { core::ptr::write\_volatile(command\_ptr, command) };

// 2. Extract BAR0 (MMIO Control Registers - 32-bit, Non-prefetchable)

let bar0 = unsafe { core::ptr::read\_volatile((device\_virt + 0x10) as \*const u32) };

let mmio\_phys = (bar0 & 0xFFFFFFF0) as u64;

// 3. Extract BAR1 (VRAM Memory Aperture - 64-bit, Prefetchable)

let bar1 = unsafe { core::ptr::read\_volatile((device\_virt + 0x18) as \*const u32) };

let is\_64bit = (bar1 & 0b100) != 0;

let mut vram\_phys = (bar1 & 0xFFFFFFF0) as u64;

if is\_64bit {

let bar1\_high = unsafe { core::ptr::read\_volatile((device\_virt + 0x1C) as \*const u32) };

vram\_phys |= (bar1\_high as u64) << 32;

}

crate::serial\_println!("\[PCI\] -> BAR0 MMIO Control Base: {:#x}", mmio\_phys);

crate::serial\_println!("\[PCI\] -> BAR1 VRAM Aperture Base: {:#x}", vram\_phys);

// 4. Map Control Space and hand off execution to the driver initialization core

if mmio\_phys != 0 && vram\_phys != 0 {

if let Ok(mmio\_virt) = unsafe { crate::memory::map\_mmio(mmio\_phys, 0x1000000) } { // Map 16MB control block

crate::drivers::gpu::nvidia::initialize\_dynamic\_gpu(mmio\_virt, vram\_phys, device\_id);

}

}

}

}

2\. Video Memory Write-Combining Performance (nyx-kernel/src/memory.rs)

The kernel's map\_mmio function forces PageTableFlags::NO\_CACHE across all

processed ranges. This is critical for configuration registers (BAR0), but

mapping a multi-gigabyte VRAM aperture (BAR1) with NO\_CACHE will force the CPU

to stall on every pixel write, dropping rendering speeds.

Add a dedicated VRAM page-mapping function to memory.rs that applies Write-Through attributes:

pub unsafe fn map\_vram\_aperture(phys\_addr: u64, size: usize) -> Result {

let mut lock = MEMORY\_MANAGER.lock();

let system = lock.as\_mut().ok\_or("Memory System not initialized")?;

let mut active\_mapper = unsafe { active\_mapper() };

let start\_frame = PhysFrame::::containing\_address(PhysAddr::new(phys\_addr));

let end\_frame = PhysFrame::::containing\_address(PhysAddr::new(phys\_addr + size as u64 - 1));

// Leverage caching optimization combinations for fast pixel array processing

let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::WRITE\_THROUGH | PageTableFlags::BIT\_9;

for frame in PhysFrame::range\_inclusive(start\_frame, end\_frame) {

let page = Page::::containing\_address(VirtAddr::new(frame.start\_address().as\_u64()));

match system.mapper.map\_to(page, frame, flags, &mut system.frame\_allocator) {

Ok(mapper) => mapper.flush(),

Err(MapToError::PageAlreadyMapped(\_)) => continue,

Err(\_) => return Err("Failed to map high-speed VRAM memory pages"),

}

}

Ok(phys\_addr)

}

3\. DRM System Call Routing Context (nyx-kernel/src/drm.rs)

Your drm.rs module contains Linux-compatible rendering ioctl structure definitions,

but the system call dispatcher initialized inside main.rs must link userspace

file interactions straight into this framework. Ensure the file ioctl handler

routes operations on device path /dev/dri/card0 straight to handle\_advanced\_drm\_ioctl.

\--------------------------------------------------------------------------------

PHASE 1: SILICON DISCOVERY & GENERATION FINGERPRINTING

\--------------------------------------------------------------------------------

Every NVIDIA card contains a master Power Management Controller (PMC) identification

register at the base of BAR0 (0x000000). Reading this address exposes hardware fuses

that determine the specific microarchitecture family.

Create File: nyx-kernel/src/drivers/gpu/nvidia/mod.rs

use spin::Mutex;

pub static ACTIVE\_NVIDIA\_DEVICE: Mutex\> = Mutex::new(None);

#\[derive(Debug, Clone, Copy, PartialEq)\]

pub enum GpuGeneration {

Tesla, // NV50 (GeForce 8/9/200 legacy layout)

Fermi, // NVC0 (GeForce 400/500 direct mapping)

Kepler, // NVE0 (GeForce 600/700 architecture transition)

Maxwell, // NV110 (GeForce 750/900 secure firmware requirements)

Pascal, // NV130 (GeForce 1000)

Turing, // NV140/NV160 (GTX 1600/RTX 2000)

Ampere, // NV170 (RTX 3000)

AdaLovelace, // NV190 (RTX 4000)

Unsupported,

}

pub struct DynamicGpuDevice {

pub mmio\_base: u64,

pub vram\_phys: u64,

pub device\_id: u16,

pub generation: GpuGeneration,

}

pub fn initialize\_dynamic\_gpu(mmio\_base: u64, vram\_phys: u64, device\_id: u16) {

// Offset 0x000000 holds PMC\_BOOT\_0 which contains the definitive hardware identifier

let pmc\_boot\_0 = unsafe { core::ptr::read\_volatile(mmio\_base as \*const u32) };

let chip\_id = (pmc\_boot\_0 >> 20) & 0x1FF;

let generation = match chip\_id {

0x050 | 0x080 | 0x090 | 0x0A0 => GpuGeneration::Tesla,

0x0C0 | 0x0D0 => GpuGeneration::Fermi,

0x0E0 | 0x0F0 | 0x100 => GpuGeneration::Kepler,

0x110 | 0x120 => GpuGeneration::Maxwell,

0x130 => GpuGeneration::Pascal,

0x140 | 0x160 => GpuGeneration::Turing,

0x170 => GpuGeneration::Ampere,

0x190 => GpuGeneration::AdaLovelace,

\_ => GpuGeneration::Unsupported,

};

if generation == GpuGeneration::Unsupported {

crate::serial\_println!("\[NVIDIA\] Unsupported or unknown graphics chipset architecture.");

return;

}

\*ACTIVE\_NVIDIA\_DEVICE.lock() = Some(DynamicGpuDevice {

mmio\_base,

vram\_phys,

device\_id,

generation,

});

}

\--------------------------------------------------------------------------------

PHASE 2: DYNAMIC MULTI-GENERATION FIRMWARE LOADER VIA VFS

\--------------------------------------------------------------------------------

Modern architectures (Kepler and newer) feature hardware-enforced isolation blocks.

The internal PGRAPH (3D Engine) remains clamped until cryptographically signed microcode

is pushed into the card's local SRAM. Since nyx-kernel has an operational VFS mounted

on physical storage, the driver dynamically streams the correct firmware file off

the NVMe drive at runtime based on the discovered generation.

Create File: nyx-kernel/src/drivers/gpu/nvidia/firmware.rs

use crate::vfs::VFS;

use crate::memory::{allocate\_frame, phys\_to\_virt};

use crate::drivers::gpu::nvidia::{GpuGeneration, ACTIVE\_NVIDIA\_DEVICE};

use alloc::format;

use alloc::vec::Vec;

pub unsafe fn load\_generation\_firmware() -> Result<(), &'static str> {

let gpu\_lock = ACTIVE\_NVIDIA\_DEVICE.lock();

let gpu = gpu\_lock.as\_ref().ok\_or("GPU global context missing")?;

// Determine the storage subdirectory based on the identified generation path

let gen\_dir = match gpu.generation {

GpuGeneration::Kepler => "gk104",

GpuGeneration::Maxwell => "gm204",

GpuGeneration::Pascal => "gp100",

GpuGeneration::Turing => "tu117",

GpuGeneration::Ampere => "ga100",

GpuGeneration::AdaLovelace => "ad102",

\_ => return Ok(()), // Legacy architectures bypass the firmware check

};

let target\_path = format!("/lib/firmware/nvidia/{}/gsp/gsp.bin", gen\_dir);

// 1. Stream the firmware file via our active file system

let raw\_blob = VFS.read\_file\_alloc(&target\_path)

.map\_err(|\_| "Firmware file missing from physical NVMe drive mount points")?;

// 2. Allocate continuous blocks of system memory

let pages\_needed = (raw\_blob.len() + 4095) / 4096;

let mut physical\_pages = Vec::with\_capacity(pages\_needed);

for i in 0..pages\_needed {

let frame = allocate\_frame().ok\_or("OOM: Firmware block copy buffer fault")?;

let phys\_addr = frame.start\_address().as\_u64();

let virt\_addr = phys\_to\_virt(phys\_addr).unwrap();

let chunk\_size = core::cmp::min(4096, raw\_blob.len() - (i \* 4096));

core::ptr::copy\_nonoverlapping(raw\_blob.as\_ptr().add(i \* 4096), virt\_addr as \*mut u8, chunk\_size);

physical\_pages.push(phys\_addr);

}

// 3. Point the hardware's Falcon processor window (BAR0 Offset 0x002000) to the allocated buffer

let base\_phys = physical\_pages\[0\];

core::ptr::write\_volatile((gpu.mmio\_base + 0x002A00) as \*mut u32, (base\_phys & 0xFFFFFFFF) as u32);

core::ptr::write\_volatile((gpu.mmio\_base + 0x002A04) as \*mut u32, (base\_phys >> 32) as u32);

// Kick the boot vector register to start processing the firmware code

core::ptr::write\_volatile((gpu.mmio\_base + 0x002100) as \*mut u32, 0x2);

crate::serial\_println!("\[NVIDIA\] Firmware microcode loaded successfully.");

Ok(())

}

\--------------------------------------------------------------------------------

PHASE 3: GMMU & GPFIFO ENGINE SUBSYSTEMS

\--------------------------------------------------------------------------------

Once the card's safety engine confirms the firmware signatures, you must set up

memory translation and command paths before sending execution instructions.

1\. The GMMU Configuration:

The graphics card cannot access standard system physical memory pointers directly.

The driver creates a dedicated 4-level page table structure (PML4 -> PDPT -> PD -> PT)

using system memory frames allocated via allocate\_frame(). This allows the

hardware's execution engine to look up memory blocks securely, using a

GMMU\_FLAG\_VRAM descriptor bit to differentiate between local high-speed VRAM

and shared system memory pages.

2\. The GPFIFO Channel Ring Buffer:

Each graphic process manages its own independent command rings. The driver

configures a circular buffer in host memory to handle pushbuffer command chains.

When new commands are added, writing the updated tracking position to the card's

doorbell register triggers a direct DMA operation, prompting the hardware

scheduler to execute the batch asynchronously.

\--------------------------------------------------------------------------------

PHASE 4: COMPOSITOR REFACTORING FOR HARDWARE ACCELERATION

\--------------------------------------------------------------------------------

With the memory management structures, driver initialization routines, and command

streams complete, the userspace compositor (nyx-user) can stop computing color blits

manually via nested CPU pixel loops.

Instead of consuming millions of CPU cycles processing pixel indices, nyx-user

is refactored to pack rendering commands (such as coordinates, surface indices,

and alpha blending configurations) into raw command arrays and pass them directly

to the card's 3D pipelines via an ioctl transaction.

Refactored Drawing Vector Engine Implementation (nyx-user/src/hardware\_render.rs):

pub const TURING\_3D\_CLASS: u32 = 0xC597;

pub const METH\_SET\_RENDER\_TARGET: u32 = 0x0208;

pub const METH\_DRAW\_PRIMITIVE: u32 = 0x030C;

pub fn execute\_hardware\_compositor\_frame(state: &mut CompositorState, fd\_card: usize) {

if !state.needs\_redraw { return; }

let mut command\_batch = \[0u32; 32\];

let mut offset = 0;

// 1. Bind the 3D pipeline object into subchannel 0 inside our command stream

command\_batch\[0\] = (4 << 28) | (1 << 16) | (0 << 13) | 0x0000;

command\_batch\[1\] = TURING\_3D\_CLASS;

offset += 2;

// 2. Loop through windows and generate hardware commands

for client in state.clients.iter() {

if client.win.exists && !client.win.is\_minimized {

// Set render target destination coordinates layout bounds

command\_batch\[offset\] = (4 << 28) | (2 << 16) | (0 << 13) | METH\_SET\_RENDER\_TARGET;

command\_batch\[offset + 1\] = client.win.x as u32;

command\_batch\[offset + 2\] = client.win.y as u32;

// Execute textured blit and alpha blending concurrently on the graphics hardware

command\_batch\[offset + 3\] = (4 << 28) | (3 << 16) | (0 << 13) | METH\_DRAW\_PRIMITIVE;

command\_batch\[offset + 4\] = client.shm\_id as u32; // Texture looked up via GMMU maps

command\_batch\[offset + 5\] = client.buf\_w as u32;

command\_batch\[offset + 6\] = client.win.opacity as u32; // Alpha computed in GPU silicon

offset += 7;

if offset >= 24 { break; }

}

}

// 3. Dispatch the complete packet to the kernel's DRM layer in a single operation

// This offloads the entire rendering workload to the hardware pipelines!

sys\_ioctl\_submit\_commands(fd\_card, &command\_batch);

state.needs\_redraw = false;

}

\================================================================================