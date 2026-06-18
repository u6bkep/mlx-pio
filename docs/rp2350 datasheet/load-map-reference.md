# RP2350 LOAD_MAP Reference

This document provides an authoritative reference for implementing LOAD_MAP items in RP2350 block loops. Information is verified against multiple sources: the RP2350 datasheet, bootrom source code, and official picotool implementation.

## Project Use Cases

This project has two primary LOAD_MAP scenarios:

1. **Bootloader (Packaged RAM Binary)**: Secure boot, stored in flash, executed from RAM, A/B updated
2. **Application (XIP Binary)**: Stored in flash, executed in place via address translation, A/B updated

Both use cases leverage the LOAD_MAP's physical address semantics to enable A/B updates with **identical IMAGE_DEF blocks** between the A and B partitions.

## Purpose

The LOAD_MAP item in an IMAGE_DEF block serves two critical functions:

1. **Defines what data to hash**: For signed/hashed binaries, the LOAD_MAP specifies which memory regions are included in the hash computation.

2. **Specifies loading behavior**: For packaged binaries (flash-resident but RAM-executed), the LOAD_MAP tells the bootrom where to copy data before execution.

## Sources of Truth

| Source | Location | Verification Status |
|--------|----------|---------------------|
| RP2350 Datasheet Section 5.9.3.2 | Official Raspberry Pi documentation | ✅ Primary reference |
| Bootrom source `varm_blocks.c` | `reference/pico-bootrom-rp2350/src/main/arm/varm_blocks.c` | ✅ Implementation reference |
| Picotool `metadata.h` | `reference/picotool/bintool/metadata.h` | ✅ Tool compatibility reference |
| Pico SDK `picobin.h` | `reference/pico-sdk/src/common/boot_picobin_headers/include/boot/picobin.h` | ✅ Data structure reference |

---

## Key Concept: Physical vs Storage Addresses

### The Critical Distinction

The RP2350 uses **three different address concepts** that are often confused:

| Term | Definition | Example |
|------|------------|---------|
| **Physical Address (LMA)** | Where data appears in the *logical* address space of the image. This is what the linker produces. | `0x10000000` (flash base) or `0x10000200` (flash + offset) |
| **Storage Address** | The *actual* location in flash where data is stored. Depends on partition. | `0x10003200` (bootloader_a partition + offset) |
| **Runtime Address (VMA)** | Where data will be at execution time. For RAM binaries, this is RAM. For XIP, same as physical. | `0x20000000` (RAM) or `0x10000000` (XIP) |

### Datasheet Quote (Section 5.9.3.2)

> "RP2350 uses **physical addresses** in the LOAD_MAP, not storage addresses, since this data is written by a tool working on the ELF which will not necessarily know where the binary will finally be stored in flash."

### Why This Matters

This design enables **A/B partition updates**:
- Both `bootloader_a` (at `0x10003000`) and `bootloader_b` (at `0x10009000`) use **identical** LOAD_MAP entries with physical address `0x10000200`
- Both `app_a` (at `0x1000F000`) and `app_b` (at `0x10104000`) use **identical** LOAD_MAP entries with physical address `0x10000000`
- The bootrom calculates the actual storage location at boot time based on which partition is being booted

---

## Bootrom Address Translation

### The `lma_to_storage` Calculation

The bootrom translates physical addresses to storage addresses using this calculation (from `varm_blocks.c` lines ~907-920):

```c
// Calculate the translation offset
uint32_t lma_to_storage = parsed_block->enclosing_window.base 
                        + parsed_block->slot_roll 
                        - XIP_BASE 
                        + ((const parsed_image_def_t *)parsed_block)->rolling_window_delta;

// Apply translation to LOAD_MAP storage addresses
if (!call_varm_is_sram_or_xip_ram(from_storage_addr)) {
    from_storage_addr += lma_to_storage;
}
```

### Understanding the Variables

| Variable | Description | Source |
|----------|-------------|--------|
| `enclosing_window.base` | Start of the partition/window being scanned | Set during partition scanning |
| `slot_roll` | Adjustment for slot 1 in dual-slot configs | Set in `varm_flash_boot.c` line 540 |
| `XIP_BASE` | Flash base address (`0x10000000`) | Hardware constant |
| `rolling_window_delta` | Custom address translation offset | From ROLLING_WINDOW_DELTA item in IMAGE_DEF |

### Example Calculation

For a bootloader stored at partition `0x10003000` with binary at offset `0x200`:

```
LOAD_MAP storage_addr = 0x10000200 (physical address from IMAGE_DEF)
enclosing_window.base = 0x10003000 (partition start)
slot_roll = 0 (no slot rolling)
XIP_BASE = 0x10000000
rolling_window_delta = 0 (no custom translation)

lma_to_storage = 0x10003000 + 0 - 0x10000000 + 0 = 0x3000

actual_storage_addr = 0x10000200 + 0x3000 = 0x10003200 ✓
```

---

## LOAD_MAP Item Format

### Binary Structure

Per datasheet Section 5.9.3.2:

| Word | Bytes | Content |
|------|-------|---------|
| 0 | byte 0 | `0x06` (ITEM_2BS_LOAD_MAP) |
| 0 | byte 1 | Size low byte (1 + num_entries × 3) |
| 0 | byte 2 | Size high byte |
| 0 | byte 3 | `absolute:1` \| `num_entries:7` |
| 1 | 4 bytes | Entry 0: storage_address (or relative offset if !absolute) |
| 2 | 4 bytes | Entry 0: runtime_address |
| 3 | 4 bytes | Entry 0: size (relative) or runtime_end_address (absolute) |
| 4-6 | ... | Entry 1 (if present) |
| ... | ... | Additional entries |

### Absolute vs Relative Mode

**Absolute mode** (`absolute = 1`, bit 31 of word 0 byte 3 set):
- `storage_address`: Absolute physical address (e.g., `0x10000200`)
- `runtime_address`: Absolute runtime address (e.g., `0x20000000`)
- Third word: `runtime_end_address = runtime_address + size`

**Relative mode** (`absolute = 0`):
- `storage_address_rel`: Offset relative to this LOAD_MAP item's address
- `runtime_address`: Absolute runtime address
- Third word: `size` in bytes

**Recommendation**: Use **absolute mode** for clarity and consistency with picotool.

### Special Cases

#### Zero storage_address
If `storage_address == 0x00000000`:
- The runtime region is filled with zeros (BSS-style initialization)
- The 32-bit size value itself is hashed (not `size` zero bytes)
- Source: Datasheet Table 5.9.3.2 and bootrom `varm_blocks.c` lines ~960-970

#### XIP (Execute in Place)
When `storage_address == runtime_address`:
- No copy occurs; data is hashed in place
- Used for flash-resident XIP binaries
- Still requires LOAD_MAP for hash definition

---

## Use Case 1: Secure Bootloader (Packaged RAM Binary)

### Overview

The bootloader is a **packaged binary**:
- **Storage**: Flash partition (`bootloader_a` at `0x10003000` or `bootloader_b` at `0x10009000`)
- **Execution**: RAM (`0x20000000`)
- **Security**: Signed for secure boot (signature verified before execution)
- **Updates**: A/B partitions with identical IMAGE_DEF blocks

### Why Packaged Binary?

Per datasheet Section 5.10.2:
> "For secure boot, it is recommended to use packaged SRAM binaries instead of flash binaries, as the signature check is only performed during boot, so a malicious actor with physical access could replace the data on the external flash after the signature check to run unsigned code."

The bootrom:
1. Copies the binary from flash to RAM
2. Hashes the RAM copy during the copy operation
3. Verifies the signature against the hash
4. Executes from the verified RAM copy

This eliminates TOCTOU (Time-Of-Check-Time-Of-Use) vulnerabilities.

### LOAD_MAP Specification

**Source**: Datasheet Section 5.10.3

```
LOAD_MAP entry:
  storage_addr:  0x10000200  (physical address: flash_base + BINARY_OFFSET)
  runtime_addr:  0x20000000  (RAM base)
  size:          <binary_size>
```

**Critical**: The `storage_addr` is a **physical address** (LMA from the ELF), NOT the partition address. The bootrom calculates actual storage location:

```
actual_storage = storage_addr + (partition_base - XIP_BASE + rolling_window_delta)
```

### Bootrom Processing (varm_blocks.c lines 900-1060)

```c
// 1. Read LOAD_MAP entry
uint32_t map_vma = entry->runtime_address;           // 0x20000000
uint32_t map_storage_address_value = entry->storage_address_rel;  // 0x10000200

// 2. Calculate translation offset
uint32_t lma_to_storage = parsed_block->enclosing_window.base  // partition start
                        + parsed_block->slot_roll 
                        - XIP_BASE                              // 0x10000000
                        + image_def->rolling_window_delta;

// 3. Translate storage address (for flash addresses only)
if (map_vma < XIP_BASE + 16*1024*1024) {  // Flash region
    to_storage_addr += lma_to_storage;
}

// 4. Copy from flash to RAM (if different addresses)
if (to_storage_addr != from_storage_addr) {
    call_s_varm_crit_mem_copy_by_words(
        (uint32_t *)to_storage_addr,    // RAM dest
        from_storage_addr,               // Flash src (translated)
        size
    );
}

// 5. Hash the destination (RAM) data
sb_sha256_update_32(&sha, src, size);
```

### A/B Update Correctness

Both `bootloader_a` and `bootloader_b` use **identical** IMAGE_DEF blocks:

| Field | Value | Notes |
|-------|-------|-------|
| storage_addr | `0x10000200` | Same for both partitions |
| runtime_addr | `0x20000000` | Same for both partitions |
| Signature | `<sig>` | Same for both partitions |

**Why this works**:
- Partition A at `0x10003000`: `lma_to_storage = 0x10003000 - 0x10000000 = 0x3000`
  - Actual storage: `0x10000200 + 0x3000 = 0x10003200` ✓
- Partition B at `0x10009000`: `lma_to_storage = 0x10009000 - 0x10000000 = 0x9000`
  - Actual storage: `0x10000200 + 0x9000 = 0x10009200` ✓

The binary content at each partition is identical, so the hash/signature remains valid.

### Security Properties

1. **Integrity**: Hash covers the binary data that will execute
2. **Authenticity**: Signature verified before execution
3. **TOCTOU-safe**: Binary is copied to RAM, then hashed, then verified
4. **Anti-rollback**: VERSION item can include rollback protection (optional)

---

## Use Case 2: Application (XIP Binary with Address Translation)

### Overview

The application is an **XIP (Execute-In-Place) binary**:
- **Storage**: Flash partition (`app_a` at `0x1000F000` or `app_b` at `0x10104000`)
- **Execution**: Flash via hardware address translation (appears at `0x10000000`)
- **Security**: Optionally hashed/signed (but NOT secure against physical attacks)
- **Updates**: A/B partitions with identical IMAGE_DEF blocks

### How Address Translation Works

Per datasheet Section 5.1.19:
> "When launching an image from a partition, the bootrom initialises QMI registers ATRANS0 through ATRANS3 to map a flash runtime address of 0x10000000 (by default) to the flash storage address of the start of the partition."

The RP2350 QMI hardware provides address translation:
- Application is linked at `0x10000000` (standard flash base)
- At runtime, accesses to `0x10000000` are transparently translated to actual partition location
- Application code doesn't need to know its actual storage location

### LOAD_MAP Specification

**Source**: Datasheet Section 5.9.3.2

```
LOAD_MAP entry:
  storage_addr:  0x10000000  (physical address: flash base - where app thinks it is)
  runtime_addr:  0x10000000  (same - XIP, no copy needed)
  size:          <binary_size>
```

### Bootrom Processing (varm_launch_image.c lines 225-255)

```c
// Calculate roll offset for ATRANS setup
int32_t roll = parsed_block_loop->flash_start_offset + image_def->rolling_window_delta;

// Set up ATRANS registers to map partition to 0x10000000
for (uint i = 0; i < 4; i++) {
    int32_t this_size = MIN(size, 0x400);  // 4MB per register
    qmi_hw->atrans[i] = (uint)((this_size << 16) | roll);
    size -= this_size;
    roll += this_size;
}
```

Example for `app_a` at `0x1000F000`:
```
ATRANS0: maps 0x10000000-0x103FFFFF → 0x1000F000-0x1040EFFF
ATRANS1: maps 0x10400000-0x107FFFFF → 0x1040F000-0x1080EFFF (if needed)
```

### Hash Verification for XIP

For XIP binaries, the bootrom hashes data **in-place** from flash:

```c
// varm_blocks.c line 1059-1060
if (to_storage_addr == from_storage_addr) {
    // No copy - hash in place
    hx_assert_equal2i(to_storage_addr, from_storage_addr_check);
}

// Hash the data at storage location
sb_sha256_update_32(&sha, src, size);
```

**Important**: The bootrom hashes from the **actual storage location**, not the translated address. The `lma_to_storage` translation is applied to find the real data.

### A/B Update Correctness

Both `app_a` and `app_b` use **identical** IMAGE_DEF blocks:

| Field | Value | Notes |
|-------|-------|-------|
| storage_addr | `0x10000000` | Same for both partitions |
| runtime_addr | `0x10000000` | Same for both partitions |
| Hash/Signature | `<hash>` | Same for both partitions |

**Why this works**:
- Partition A at `0x1000F000`: `lma_to_storage = 0x1000F000 - 0x10000000 = 0xF000`
  - Hash reads from: `0x10000000 + 0xF000 = 0x1000F000` ✓
  - ATRANS maps: `0x10000000` → `0x1000F000` ✓
- Partition B at `0x10104000`: `lma_to_storage = 0x10104000 - 0x10000000 = 0x104000`
  - Hash reads from: `0x10000000 + 0x104000 = 0x10104000` ✓
  - ATRANS maps: `0x10000000` → `0x10104000` ✓

### Security Considerations

**XIP binaries are NOT secure against physical attacks**. Per datasheet Section 5.10.2:
> "The signature check is only performed during boot, so a malicious actor with physical access could replace the data on the external flash after the signature check to run unsigned code."

XIP binaries are appropriate when:
- Physical security is ensured (tamper-evident enclosure)
- The threat model doesn't include flash replacement attacks
- Performance/size requirements preclude RAM execution

For maximum security, the bootloader should be a packaged RAM binary, and if the app needs secure boot, consider:
1. Using a packaged RAM binary for security-critical code
2. Re-verifying signatures at runtime for critical operations
3. Using flash encryption (not covered here)

---

## Implementation Examples

### XIP Binary (Execute from Flash)

For an XIP binary linked at `0x10000000`:

```rust
// LOAD_MAP entry for XIP
let storage_addr = 0x10000000;  // Physical address (flash base)
let runtime_addr = 0x10000000;  // Same - execute in place
let size = binary.len() as u32;

builder.add_load_map(true, &[(storage_addr, runtime_addr, size)]);
```

**Result**: Both app_a and app_b partitions use this same LOAD_MAP. The bootrom:
1. Sets up ATRANS registers to map the partition to `0x10000000`
2. Hashes the data at the actual storage location
3. Executes from the translated address `0x10000000`

### Packaged Binary (Flash to RAM)

For a bootloader stored at flash offset `0x200` but executing from RAM:

```rust
// LOAD_MAP entry for packaged RAM binary
let storage_addr = 0x10000200;  // Physical address = flash_base + offset
let runtime_addr = 0x20000000;  // RAM execution address
let size = binary.len() as u32;

builder.add_load_map(true, &[(storage_addr, runtime_addr, size)]);
```

**Result**: The bootrom:
1. Reads `storage_addr` from LOAD_MAP: `0x10000200`
2. Calculates `lma_to_storage` from partition info: e.g., `0x3000`
3. Computes actual flash address: `0x10000200 + 0x3000 = 0x10003200`
4. Copies data from `0x10003200` to `0x20000000`
5. Hashes the copied data
6. Executes from RAM at `0x20000000`

### BSS Clear (Zero-Fill RAM Region)

To zero-fill a RAM region:

```rust
// Clear all SRAM before loading (security measure)
let storage_addr = 0x00000000;  // Special: zero-fill
let runtime_addr = 0x20000000;  // RAM start
let size = 0x00082000;          // SRAM size

builder.add_load_map(true, &[(storage_addr, runtime_addr, size)]);
// Then add the actual binary load map entry
```

---

## Verification Checklist

When implementing LOAD_MAP generation, verify:

- [ ] Item type is `0x06` (ITEM_2BS_LOAD_MAP)
- [ ] Size field = 1 + (num_entries × 3) words
- [ ] Absolute bit (bit 7 of byte 3) set appropriately
- [ ] Entry count (bits 0-6 of byte 3) is correct
- [ ] **Storage addresses are PHYSICAL (LMA), not actual partition storage addresses**
- [ ] For absolute mode: third word is `runtime_addr + size`
- [ ] All addresses are word-aligned (4-byte aligned)
- [ ] All sizes are multiples of 4 bytes

---

## Common Mistakes

### 1. Using Storage Address Instead of Physical Address

**Wrong**:
```rust
// DON'T DO THIS - using actual partition address
let storage_addr = 0x10003200;  // Actual flash location ❌
```

**Correct**:
```rust
// Use physical address (as if image starts at flash base)
let storage_addr = 0x10000200;  // Physical offset from flash base ✓
```

### 2. Confusing Absolute Mode Third Word

**Wrong** (picotool absolute mode):
```rust
// For absolute mode, third word is runtime_END, not size
entries.push(storage_addr);
entries.push(runtime_addr);
entries.push(size);  // ❌ Wrong for absolute mode
```

**Correct**:
```rust
entries.push(storage_addr);
entries.push(runtime_addr);
entries.push(runtime_addr + size);  // ✓ runtime_end_address
```

### 3. Non-Aligned Addresses or Sizes

Per bootrom (`varm_blocks.c`):
```c
bool valid_to_address_span =
    !(to_storage_addr & 3u) && !(size & 3) && ...
```

All addresses and sizes must be 4-byte aligned.

---

## Hash Computation with LOAD_MAP

When computing the hash for signing:

1. **LOAD_MAP regions first**: Hash the data from each LOAD_MAP entry in order
2. **Block metadata second**: Hash the specified number of block words (from HASH_DEF)

Per datasheet Section 5.10.3 and bootrom `varm_blocks.c`:

```c
// For each LOAD_MAP entry:
if (storage_address == 0) {
    // Zero-fill: hash the size value itself (4 bytes)
    sha256_update(&hash, &size, 4);
} else {
    // Normal: hash the actual data
    sha256_update(&hash, data_ptr, size);
}

// Then hash block words (from HASH_DEF.block_words_hashed)
sha256_update(&hash, block_data, block_words_hashed * 4);
```

---

## References

### Datasheet Sections
- Section 5.1.9: Load maps (conceptual overview)
- Section 5.1.10: Packaged binaries (why RAM execution is more secure)
- Section 5.1.19: Address translation (ATRANS hardware)
- Section 5.9.3.2: LOAD_MAP item format (binary structure)
- Section 5.10.2: Signed images (security recommendations)
- Section 5.10.3: Packaged binaries (examples)

### Bootrom Source Files
- `varm_blocks.c` lines 900-1100: LOAD_MAP processing, `lma_to_storage` calculation, hash computation
- `varm_launch_image.c` lines 225-255: ATRANS register setup for XIP
- `varm_boot_path.h`: `parsed_block_t` structure with `enclosing_window`, `slot_roll`
- `varm_flash_boot.c` line 540: `slot_roll` assignment during partition scanning

### Tool Implementations
- Picotool `metadata.h` lines 375-440: `load_map_item` class
- Picotool `bintool.cpp` lines 690-800: `get_lm_hash_data()` function, ELF→LOAD_MAP conversion
- Pico SDK `picobin.h` lines 156-170: `picobin_load_map` C structure

### Key Quotes

**Datasheet 5.9.3.2** (Physical Address Semantics):
> "RP2350 uses physical addresses in the LOAD_MAP, not storage addresses, since this data is written by a tool working on the ELF which will not necessarily know where the binary will finally be stored in flash."

**Datasheet 5.1.19** (Address Translation):
> "When launching an image from a partition, the bootrom initialises QMI registers ATRANS0 through ATRANS3 to map a flash runtime address of 0x10000000 (by default) to the flash storage address of the start of the partition."

**Datasheet 5.10.2** (Security Recommendation):
> "For secure boot, it is recommended to use packaged SRAM binaries instead of flash binaries, as the signature check is only performed during boot, so a malicious actor with physical access could replace the data on the external flash after the signature check to run unsigned code."

---

## Version History

| Date | Change |
|------|--------|
| 2026-01-16 | Added detailed use case documentation for bootloader (packaged RAM) and app (XIP) |
| 2026-01-16 | Initial version based on comprehensive source code analysis |
