#include "ext4.h"
#include "ext4_mbr.h"
#include <stdint.h>
#include <stdbool.h>

// 1. These are the Rust NVMe functions we will call from C
extern bool nyx_nvme_read_block(uint64_t sector, uint8_t* buf);
extern bool nyx_nvme_write_block(uint64_t sector, const uint8_t* buf);

// 2. Map lwext4 block requests to your Rust NVMe driver
static int bridge_bread(struct ext4_blockdev *bdev, void *buf, uint64_t blk_id, uint32_t blk_cnt) {
    uint8_t* p = (uint8_t*)buf;
    for (uint32_t i = 0; i < blk_cnt; i++) {
        if (!nyx_nvme_read_block(blk_id + i, p + (i * 512))) return EIO;
    }
    return EOK;
}

static int bridge_bwrite(struct ext4_blockdev *bdev, const void *buf, uint64_t blk_id, uint32_t blk_cnt) {
    const uint8_t* p = (const uint8_t*)buf;
    for (uint32_t i = 0; i < blk_cnt; i++) {
        if (!nyx_nvme_write_block(blk_id + i, p + (i * 512))) return EIO;
    }
    return EOK;
}

// Provide dummy open/close functions for safety
static int bridge_open(struct ext4_blockdev *bdev) { return EOK; }
static int bridge_close(struct ext4_blockdev *bdev) { return EOK; }

// 🔥 FIX: The new hardware interface layout expected by modern lwext4
static struct ext4_blockdev_iface nyx_bdif = {
    .open = bridge_open,
    .bread = bridge_bread,
    .bwrite = bridge_bwrite,
    .close = bridge_close,
    .ph_bsize = 512,
    .ph_bcnt = 0, // Set dynamically during mount
};

// 🔥 FIX: The main block device struct now just points to the interface
static struct ext4_blockdev nyx_bdev = {
    .bdif = &nyx_bdif,
    .part_offset = 0,
    .part_size = 0,
};

// ==========================================
// THE API EXPOSED TO RUST
// ==========================================

//  FIX: Now returns 'int' instead of 'bool'
// Add a tracker variable
static bool is_dev_registered = false;

int nyx_fs_mount(uint64_t partition_start_sector, uint64_t total_sectors) {
    nyx_bdev.part_offset = partition_start_sector * 512;
    nyx_bdev.part_size = total_sectors * 512;
    nyx_bdif.ph_bcnt = total_sectors;

    // Only register the device the very first time
    if (!is_dev_registered) {
        ext4_device_register(&nyx_bdev, "nx");
        is_dev_registered = true;
    }

    // Return the exact POSIX error code directly to Rust! (0 = Success)
    return ext4_mount("nx", "/mnt/", false);
}

int nyx_fs_read_file(const char* path, uint32_t offset, uint8_t* buf, uint32_t len) {
    ext4_file f;
    if (ext4_fopen(&f, path, "r") != EOK) return 0;
    
    ext4_fseek(&f, offset, SEEK_SET);
    size_t bytes_read = 0;
    ext4_fread(&f, buf, len, &bytes_read);
    ext4_fclose(&f);
    
    return (int)bytes_read;
}

int nyx_fs_write_file(const char* path, uint32_t offset, const uint8_t* buf, uint32_t len) {
    ext4_file f;
    // "r+" opens for read/write. If it fails, "w+" creates it.
    if (ext4_fopen(&f, path, "r+") != EOK) {
        if (ext4_fopen(&f, path, "w+") != EOK) return 0;
    }
    
    ext4_fseek(&f, offset, SEEK_SET);
    size_t bytes_written = 0;
    ext4_fwrite(&f, buf, len, &bytes_written);
    ext4_fclose(&f);
    
    return (int)bytes_written;
}

int nyx_fs_get_size(const char* path) {
    ext4_file f;
    if (ext4_fopen(&f, path, "r") != EOK) return -1;
    int size = (int)ext4_fsize(&f);
    ext4_fclose(&f);
    return size;
}

int nyx_fs_create_dir(const char* path) {
    return ext4_dir_mk(path) == EOK ? 1 : 0;
}

// ==========================================
// DIRECTORY LISTING BRIDGE
// ==========================================
void nyx_fs_list_dir(const char* path, void (*cb)(const char*, unsigned char, void*), void* ctx) {
    ext4_dir dir;
    if (ext4_dir_open(&dir, path) != EOK) return;

    const ext4_direntry *de;
    while ((de = ext4_dir_entry_next(&dir)) != 0) {
        // Copy the non-null-terminated C-string into a safe buffer
        char name_buf[256];
        int len = de->name_length;
        if (len > 255) len = 255;
        
        for(int i = 0; i < len; i++) {
            name_buf[i] = de->name[i];
        }
        name_buf[len] = '\0'; // Null terminate it for Rust

        // Trigger the Rust Callback (Pass the name, the file type, and the memory context)
        cb(name_buf, de->inode_type, ctx);
    }
    
    ext4_dir_close(&dir);
}

int nyx_fs_create_file(const char* path) {
    ext4_file f;
    // "w+" creates an empty file for reading and writing.
    if (ext4_fopen(&f, path, "w+") != EOK) return 0;
    
    // Close it immediately since we just want to create it
    ext4_fclose(&f);
    return 1;
}

// Deletes a file (or an empty directory) from the Ext4 partition
int nyx_fs_delete_file(const char* path) {
    if (ext4_fremove(path) == EOK) {
        return 1; // Success
    }
    return 0; // Failed (e.g., file doesn't exist, or folder not empty)
}
// Forces the block cache to flush its journal to the physical NVMe drive
int nyx_fs_sync(const char* path) {
    if (ext4_cache_flush(path) == EOK) {
        return 1;
    }
    return 0;
}