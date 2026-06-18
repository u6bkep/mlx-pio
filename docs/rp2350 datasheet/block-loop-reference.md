# RP2350 Block Loop Reference

This document provides a comprehensive reference for RP2350 block loops, verified against multiple authoritative sources:

1. **RP2350 Datasheet Chapter 5** (primary specification)
2. **pico-bootrom source code** (`varm_blocks.c`) (canonical implementation)
3. **picotool** (`bintool.cpp`) (reference signing/generation tool)
4. **embassy-rp** (`block.rs`) (alternative Rust implementation)

## Table of Contents

1. [Overview](#overview)
2. [Block Structure](#block-structure)
3. [Block Loop Semantics](#block-loop-semantics)
4. [Item Types Reference](#item-types-reference)
5. [Signing and Hashing](#signing-and-hashing)
6. [LOAD_MAP Address Translation](#load_map-address-translation)
7. [A/B Partition Updates](#ab-partition-updates)
8. [Implementation Guidance](#implementation-guidance)

---

## Overview

Block loops are the RP2350's mechanism for describing executable images and partition tables. They are **cyclic linked lists of blocks** that the bootrom discovers and parses during boot.

**Key Sources:**
- Datasheet Section 5.1.5: "Blocks and Block Loops"
- Datasheet Section 5.9: "Metadata Block Details"
- Bootrom: `varm_blocks.c::s_varm_crit_parse_block()`

### Core Concepts

| Concept         | Description                                      | Source            |
| --------------- | ------------------------------------------------ | ----------------- |
| Block           | Container for metadata items, bounded by markers | Datasheet 5.1.5.1 |
| Block Loop      | Cyclic linked list of blocks                     | Datasheet 5.1.5.2 |
| IMAGE_DEF       | Block describing an executable image             | Datasheet 5.9.1   |
| PARTITION_TABLE | Block describing flash layout                    | Datasheet 5.9.4   |
| First Block     | Must be in first 4KB of flash/partition          | Datasheet 5.1.5.2 |

---

## Block Structure

Every block has the following format:

```
┌─────────────────────────────┐
│ BLOCK_MARKER_START (4 bytes)│  0xFFFFDED3
├─────────────────────────────┤
│ Item 1                      │  Variable size
├─────────────────────────────┤
│ Item 2                      │  Variable size
├─────────────────────────────┤
│ ...                         │
├─────────────────────────────┤
│ LAST Item (4 bytes)         │  Encodes total item word count
├─────────────────────────────┤
│ next_block_offset (4 bytes) │  Signed relative offset to next block
├─────────────────────────────┤
│ BLOCK_MARKER_END (4 bytes)  │  0xAB123579
└─────────────────────────────┘
```

### Magic Values

| Constant             | Value        | Source                                                    |
| -------------------- | ------------ | --------------------------------------------------------- |
| `BLOCK_MARKER_START` | `0xFFFFDED3` | Datasheet Table 466, bootrom `PICOBIN_BLOCK_MARKER_START` |
| `BLOCK_MARKER_END`   | `0xAB123579` | Datasheet Table 466, bootrom `PICOBIN_BLOCK_MARKER_END`   |

**Verification:** Both values confirmed in datasheet Table 466 AND bootrom source `varm_blocks.c` line ~50 AND embassy-rp `block.rs` constants.

### Size Limits

| Block Type      | Maximum Size          | Source                  |
| --------------- | --------------------- | ----------------------- |
| IMAGE_DEF       | 384 bytes (96 words)  | Datasheet Section 5.9.1 |
| PARTITION_TABLE | 640 bytes (160 words) | Datasheet Section 5.9.4 |

### LAST Item Format

The LAST item encodes the total number of item words (excluding markers and the LAST item itself):

```
Word: [item_type=0xFF][size_lo][size_hi][pad=0x00]
```

Where `size = size_lo | (size_hi << 8)` is the count of words for all OTHER items.

**Verified in:** Datasheet Section 5.9.1, picotool `block::to_words()`, our implementation.

---

## Block Loop Semantics

### Discovery (Bootrom Scanning)

The bootrom scans the **first 4KB** (first 4096 bytes) of flash looking for `BLOCK_MARKER_START`:

```c
// From varm_blocks.c
// Scan window is limited to first 4KB for discovery
// After finding first block, loop can extend beyond 4KB
```

**Critical Rules:**

1. **First block must have lowest address** in the loop
2. **First block must be within first 4KB** of the search window
3. Subsequent blocks can be anywhere (loop is followed via relative offsets)

**Source:** Datasheet 5.1.5.2, bootrom `s_varm_crit_init_block_scan()`

### Loop Traversal

The bootrom follows relative offsets to traverse the loop:

```
Block A @ 0x100 ──(offset=+0x3000)──► Block B @ 0x3100 ──(offset=-0x3000)──► Block A
```

A single block can form a valid loop with `offset = 0` (self-referential).

### Block Selection ("Last IMAGE_DEF Wins")

When multiple IMAGE_DEFs exist in a loop, the **last one visited** is used:

> "If multiple blocks of the same type are found in the loop, the contents of the last one dominate."
> — Datasheet Section 5.1.5.2

This is critical for signing: picotool appends a signature block at the END of the loop so it becomes the authoritative IMAGE_DEF.

**Verified in:** 
- Datasheet Section 5.1.5.2
- picotool `hash_andor_sign()` appends new block
- bootrom: last parsed IMAGE_DEF is used

### Invalid Block Loops

A block loop is **invalid** (boot fails) if:
- No `BLOCK_MARKER_START` found in first 4KB
- Loop doesn't close (relative offsets don't form a cycle)
- Block extends beyond available data
- Required items missing (e.g., IMAGE_TYPE for IMAGE_DEF)

---

## Item Types Reference

### Item Header Format

Items use one of two header formats based on their type:

**1-byte size (1BS) items:**
```
[item_type:8][size:8][type_specific:16]
```

**2-byte size (2BS) items:**
```
[item_type:8][size_lo:8][size_hi:8][type_specific:8]
```

### Item Type Table

| Type                   | Code   | Size Format | Description                    | Source            |
| ---------------------- | ------ | ----------- | ------------------------------ | ----------------- |
| `IMAGE_TYPE`           | `0x42` | 1BS         | Image type and flags           | Datasheet 5.9.2.1 |
| `VERSION`              | `0x48` | 1BS         | Version with optional rollback | Datasheet 5.9.2.1 |
| `VECTOR_TABLE`         | `0x03` | 1BS         | ARM vector table address       | Datasheet 5.9.2.1 |
| `ENTRY_POINT`          | `0x44` | 1BS         | Entry PC/SP/SP-limit           | Datasheet 5.9.2.1 |
| `LOAD_MAP`             | `0x06` | 2BS         | Address mapping for hash/copy  | Datasheet 5.9.3.2 |
| `HASH_DEF`             | `0x47` | 2BS         | Hash algorithm and scope       | Datasheet 5.9.2.2 |
| `HASH_VALUE`           | `0x4B` | 1BS         | The actual hash value          | Datasheet 5.9.2.3 |
| `SIGNATURE`            | `0x09` | 1BS         | Public key + signature         | Datasheet 5.9.2.4 |
| `PARTITION_TABLE`      | `0x0A` | 2BS         | Partition definitions          | Datasheet 5.9.4   |
| `ROLLING_WINDOW_DELTA` | `0x05` | 1BS         | Window offset for signing      | Datasheet 5.9.3.1 |
| `IGNORED`              | `0xFE` | 2BS         | Placeholder, skipped           | Datasheet 5.9.3.3 |
| `LAST`                 | `0xFF` | 2BS         | Block terminator               | Datasheet 5.9.1   |

### IMAGE_TYPE Item (0x42)

Defines the image type. **Must be first item** in an IMAGE_DEF block.

```
Word 0: [type=0x42][size=1][flags_hi:8][flags_lo:8]
```

**Flags (16 bits):**

| Bits  | Mask     | Field      | Values                                  |
| ----- | -------- | ---------- | --------------------------------------- |
| 0-3   | `0x000F` | Image Type | `0x01`=EXE, `0x02`=DATA                 |
| 4-5   | `0x0030` | Security   | `0x00`=Unspecified, `0x10`=NS, `0x20`=S |
| 8-10  | `0x0700` | CPU        | `0x000`=ARM, `0x100`=RISC-V             |
| 12-14 | `0x7000` | Chip       | `0x0000`=RP2040, `0x1000`=RP2350        |
| 15    | `0x8000` | TBYB       | Try Before You Buy flag                 |

**Verified:** Datasheet Table 469, bootrom `inline_s_is_executable()`, embassy-rp constants.

**TBYB Note:** When hashing for signature, the TBYB flag is **cleared** before hashing:

```cpp
// From picotool bintool.cpp
if (((image_type_item *)new_block->items[0].get())->flags & 0x8000) {
    DEBUG_LOG("CLEARING TBYB FLAG\n");
    assert(tmp_words[1] & 0x80000000);
    tmp_words[1] &= ~0x80000000;
}
```

This allows the same signature to work whether TBYB is set or not.

### VERSION Item (0x48)

Encodes version for A/B slot selection and optional rollback protection.

**Without rollback (2 words):**
```
Word 0: [type=0x48][size=2][pad=0][num_otp_entries=0]
Word 1: [minor:16][major:16]
```

**With rollback (2 + extra words):**
```
Word 0: [type=0x48][size][pad=0][num_otp_entries]
Word 1: [minor:16][major:16]
Word 2: [rollback:16][first_otp_row:16]
Word N: [otp_row_i:16][otp_row_i+1:16]
```

Size formula: `2 + ((num_otp_entries + 2) / 2)`

**OTP Rollback Protection:**
- Each OTP row uses "thermometer encoding" (one-time increment)
- Sum of all row values must be >= rollback version
- Used to prevent downgrade attacks

**Verified:** Datasheet Section 5.9.2.1, bootrom version parsing.

### LOAD_MAP Item (0x06)

Defines what data to hash and where to copy it.

```
Word 0: [type=0x06][size_lo][size_hi][absolute:1|num_entries:7]
Entry N (3 words each):
  Word 0: storage_addr_or_rel
  Word 1: runtime_addr
  Word 2: size_or_end_addr
```

**Entry format depends on `absolute` flag:**

| Field  | absolute=0 (relative)              | absolute=1 (absolute)  |
| ------ | ---------------------------------- | ---------------------- |
| Word 0 | Relative offset from LOAD_MAP item | Physical address (LMA) |
| Word 1 | Runtime address                    | Runtime address        |
| Word 2 | Size in bytes                      | Runtime end address    |

**Special case:** If `storage_addr` is 0, the runtime region is **zeroed** (not copied), and the size is hashed as a 4-byte word.

**Verified:** Datasheet 5.9.3.2, bootrom `s_varm_crit_ram_trash_verify_block()`, picotool `get_lm_hash_data()`.

### HASH_DEF Item (0x47)

Defines hash algorithm and scope.

```
Word 0: [type=0x47][size=2][0x00][hash_type=0x01]
Word 1: block_words_hashed
```

**hash_type:** Currently only `0x01` = SHA-256 is supported.

**block_words_hashed:** Number of words from first item (after `BLOCK_MARKER_START`) to hash from the block itself. This **includes** the HASH_DEF item and extends up to (but not including) the data words of the SIGNATURE item.

**Verified:** Datasheet 5.9.2.2, picotool `hash_andor_sign_block()`, bootrom.

### SIGNATURE Item (0x09)

Contains public key and ECDSA signature.

```
Word 0: [type=0x09][size=0x21][pad=0x00][sig_type=0x01]
Words 1-16: Public key (64 bytes, X||Y uncompressed, no 0x04 prefix)
Words 17-32: Signature (64 bytes, r||s compact format)
```

**sig_type:** Currently only `0x01` = secp256k1 is supported.

**Key format:** 64-byte uncompressed public key (32-byte X coordinate + 32-byte Y coordinate). No 0x04 SEC1 prefix.

**Verified:** Datasheet 5.9.2.4, bootrom `s_arm8_verify_signature_secp256k1()`, picotool.

### HASH_VALUE Item (0x4B)

Contains the actual hash value for verification.

```
Word 0: [type=0x4B][size=1+N][pad]
Words 1-N: Hash bytes (4-32 bytes, must be multiple of 4)
```

For SHA-256, typically 8 words (32 bytes). Shorter hashes provide less security but save space.

**Verified:** Datasheet 5.9.2.3, bootrom hash comparison logic.

---

## Signing and Hashing

### What Gets Hashed

For a signed IMAGE_DEF, the hash covers:

1. **LOAD_MAP regions** - Binary data from flash/RAM specified by LOAD_MAP entries
2. **Block items** - From first item through `block_words_hashed` words

```
Hash Input = [LOAD_MAP data regions] + [Block items up to block_words_hashed]
```

### Hash Computation Order

Per bootrom `s_varm_crit_ram_trash_verify_block()`:

1. Initialize SHA-256
2. For each LOAD_MAP entry:
   - If `storage_addr == 0`: Hash the 4-byte size value (for zero-fill regions)
   - Else: Hash the data at storage address (translated to actual location)
3. Hash block words: `sha256.update(block_data[0..block_words_hashed * 4])`
4. Finalize hash

### block_words_hashed Value

The `block_words_hashed` field in HASH_DEF must be set to include items up through the **first word of SIGNATURE** (its header word).

From picotool:
```cpp
// hash everything up to an including the hash def
auto tmp_words = new_block->to_words();
// ... remove stuff at end
auto block_hashed_contents = words_to_lsb_bytes(tmp_words.begin(), tmp_words.end() - 3);
```

The `-3` removes: `LAST item (1) + offset (1) + end_marker (1)` = 3 words.

But the SIGNATURE item comes AFTER HASH_DEF, so the formula is:

```
block_words_hashed = items_before_hash_def + hash_def_size(2) + 1 (sig header only)
```

### TBYB Flag Handling

When computing the hash, the TBYB flag (bit 15 of IMAGE_TYPE flags) is **cleared**:

```cpp
// From picotool and bootrom
if (image_type_flags & 0x8000) {
    image_type_word &= ~0x80000000;  // Clear TBYB in the word being hashed
}
```

This allows the bootrom to set/clear TBYB without invalidating the signature.

### Signature Block Placement

For multi-block signing (common pattern):

1. **Starter block** at offset 0 (within first 4KB) - minimal IMAGE_DEF
2. **Binary code** follows
3. **Signature block** at end of binary - full IMAGE_DEF with LOAD_MAP, HASH_DEF, SIGNATURE, HASH_VALUE

The signature block links back to the starter block, and because it's last in the loop, it becomes the authoritative IMAGE_DEF.

---

## LOAD_MAP Address Translation

### Critical Concept: Physical vs Storage Addresses

**This is a common source of bugs.** Per datasheet Section 5.9.3.2:

> "RP2350 uses **physical addresses** in the LOAD_MAP, not storage addresses, since this data is written by a tool working on the ELF which will not necessarily know where the binary will finally be stored in flash."

### Translation Mechanism

The bootrom translates physical addresses to actual storage addresses:

```c
// From varm_blocks.c
uint32_t lma_to_storage = parsed_block->enclosing_window.base 
                        + parsed_block->slot_roll 
                        - XIP_BASE 
                        + ((const parsed_image_def_t *)parsed_block)->rolling_window_delta;
```

For a typical case:
- `enclosing_window.base` = partition start (e.g., `0x10003000`)
- `slot_roll` = 0 (no slot roll)
- `XIP_BASE` = `0x10000000`
- `rolling_window_delta` = 0

So `lma_to_storage = 0x10003000 - 0x10000000 = 0x3000`

A LOAD_MAP entry with `storage_addr = 0x10000000` becomes actual flash offset `0x10003000`.

### Implications for A/B Updates

**Both app_a and app_b can use IDENTICAL LOAD_MAPs:**

| Partition          | Actual Location | LOAD_MAP storage_addr |
| ------------------ | --------------- | --------------------- |
| app_a @ 0x10010000 | 0x10010000      | 0x10000000            |
| app_b @ 0x10020000 | 0x10020000      | 0x10000000            |

The bootrom automatically adds the partition offset. This is why you can build a single signed image and flash it to either A or B partition.

### XIP vs Packaged Binaries

| Binary Type  | LOAD_MAP storage_addr         | LOAD_MAP runtime_addr    | Bootrom behavior                     |
| ------------ | ----------------------------- | ------------------------ | ------------------------------------ |
| **XIP**      | Physical (e.g., `0x10000000`) | Same as storage          | Sets ATRANS for address translation  |
| **Packaged** | Physical (e.g., `0x10000200`) | RAM (e.g., `0x20000000`) | Adds partition offset, copies to RAM |

For packaged binaries (like RAM bootloaders), the `storage_addr` in LOAD_MAP should still be a physical address (typically `0x10000000 + offset`), NOT the actual partition address.

---

## A/B Partition Updates

### Partition Table Structure for A/B

For A/B with updateable bootloader and app:

```
Slot 0 (0x00000000-0x00001000): PT_A + Bootloader_A header
Slot 1 (0x00001000-0x00002000): PT_B + Bootloader_B header  [optional if singleton=false]
Partition 0 (bootloader_a): 0x00002000-0x00005000
Partition 1 (bootloader_b): 0x00005000-0x00008000, link=A(0)
Partition 2 (app_a): 0x00008000-0x00040000
Partition 3 (app_b): 0x00040000-0x00078000, link=A(2)
```

### Block Loop for Custom Bootloader

Per datasheet Section 5.10.6:

```
┌─────────────────────────────────────────┐
│ Starter Block (in first 4KB)            │
│ - IMAGE_DEF (minimal, will be           │
│   superseded by later block)            │
├─────────────────────────────────────────┤
│ Binary Code                             │
│ - Vector table                          │
│ - Code                                  │
│ - Data                                  │
├─────────────────────────────────────────┤
│ PARTITION_TABLE Block                   │
│ - Partition definitions                 │
├─────────────────────────────────────────┤
│ Signed IMAGE_DEF Block                  │
│ - IMAGE_TYPE                            │
│ - VERSION                               │
│ - LOAD_MAP (covers binary + PT)         │
│ - HASH_DEF                              │
│ - SIGNATURE                             │
│ - HASH_VALUE                            │
│ ◄───────── Links back to first block    │
└─────────────────────────────────────────┘
```

### Covering Signature

The IMAGE_DEF's LOAD_MAP can "cover" the PARTITION_TABLE:

> "As long as the LOAD_MAP defined area to be hashed/signed includes the entirety of the PARTITION_TABLE block, the 'covering' signature is used to validate the PARTITION_TABLE too."
> — Datasheet Section 5.10.6

This means a single signature can protect both bootloader code AND partition table.

### Version-Based Slot Selection

For A/B pairs:
1. Bootrom checks VERSION items in both partitions
2. Higher version wins (major.minor comparison)
3. If versions equal, A partition is preferred

For bootloader slots (slot 0 vs slot 1):
- Same VERSION comparison applies
- Use `singleton=true` to disable slot 1 checking (for simpler setups)

---

## Implementation Guidance

### Minimal IMAGE_DEF (20 bytes)

Per datasheet Section 5.10.1:

```
start_marker:  0xFFFFDED3
image_type:    0x10211042  // ARM, Secure, RP2350, EXE
last:          0x000001FF  // 1 word of items
offset:        0x00000000  // self-loop
end_marker:    0xAB123579
```

This is 20 bytes and creates a valid self-looping IMAGE_DEF.

### Signed IMAGE_DEF Item Order

For a signed IMAGE_DEF, items should appear in this order:

1. `IMAGE_TYPE` (required, must be first)
2. `VERSION` (recommended)
3. `VECTOR_TABLE` (optional, if non-default)
4. `ENTRY_POINT` (optional, if non-default)
5. `LOAD_MAP` (required for signing)
6. `HASH_DEF` (required for signing)
7. `SIGNATURE` (required for signing)
8. `HASH_VALUE` (required for hash verification)

### Block Loop Construction Steps

1. **Create starter block** at offset 0 (minimal IMAGE_DEF, offset=0 for now)
2. **Place binary code** starting after starter block
3. **Create signature block** at binary end with full IMAGE_DEF
4. **Compute LOAD_MAP** covering binary regions
5. **Compute hash** over LOAD_MAP regions + block items
6. **Sign hash** with secp256k1
7. **Update offsets** to form closed loop:
   - Starter → Signature: `signature_offset - starter_offset`
   - Signature → Starter: `starter_offset - signature_offset` (negative)

### Hash Verification Checklist

When implementing hash/signature verification:

1. ✅ Use physical addresses in LOAD_MAP (bootrom translates)
2. ✅ Clear TBYB flag before hashing
3. ✅ Hash LOAD_MAP data first, then block items
4. ✅ `block_words_hashed` includes up to SIGNATURE header word
5. ✅ For zero-fill entries (`storage_addr=0`), hash the 4-byte size value
6. ✅ Use little-endian byte order for all words

### Common Pitfalls

1. **Using partition addresses in LOAD_MAP** - Use physical addresses instead
2. **Forgetting TBYB clearing** - Hash computed differently than block stored
3. **Wrong block_words_hashed** - Must include HASH_DEF + SIGNATURE header
4. **First block outside 4KB** - Bootrom won't find it
5. **Non-closed loop** - Offsets must form cycle back to first block
6. **Missing items** - IMAGE_TYPE required for IMAGE_DEF, LOAD_MAP required for signing

---

## References

### Datasheet Sections
- 5.1.5: Blocks and Block Loops
- 5.9: Metadata Block Details
- 5.9.1: IMAGE_DEF block items
- 5.9.2: Signing items (HASH_DEF, HASH_VALUE, SIGNATURE)
- 5.9.3: LOAD_MAP details
- 5.9.4: PARTITION_TABLE item
- 5.10: Example boot scenarios

### Source Files
- `reference/pico-bootrom-rp2350/src/main/arm/varm_blocks.c`
- `reference/picotool/bintool/bintool.cpp`
- `libs/embassy/embassy-rp/src/block.rs`
- `crates/rp2350-block-loop/src/lib.rs`

### Key Bootrom Functions
- `s_varm_crit_parse_block()` - Block parsing
- `s_varm_crit_ram_trash_verify_block()` - Hash/signature verification
- `s_varm_crit_next_block()` - Block loop traversal
- `s_varm_crit_init_block_scan()` - Initial block discovery
