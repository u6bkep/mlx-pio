# TBYB Bootrom Analysis — RP2350

Deep-dive into how the RP2350 bootrom handles Try Before You Buy (TBYB), based on
reading the bootrom source code in `reference/pico-bootrom-rp2350/`.

## 1. TBYB Bit in IMAGE_DEF

**Bit position:** Bit 15 of the `image_type` field (word 1 of the IMAGE_TYPE item, upper 16 bits).
Constant: `PICOBIN_IMAGE_TYPE_EXE_TBYB_BITS = 0x8000`.

**Critical bootrom behavior during block parsing** (`varm_blocks.c:413-415`):
```c
// clear tbyb flag so we don't break sig check
block_data[1] &= ~(PICOBIN_IMAGE_TYPE_EXE_TBYB_BITS << 16);
parsed_block->tbyb_flagged = image_type_flags & PICOBIN_IMAGE_TYPE_EXE_TBYB_BITS;
```

The bootrom **strips the TBYB bit from the block data** before hash/signature verification
and stores it separately in `parsed_block->tbyb_flagged`. This means:
- The hash computed over the IMAGE_TYPE item does NOT include the TBYB bit
- The signature is valid regardless of whether TBYB is set or clear
- `explicit_buy` can clear the TBYB bit without invalidating the signature


## 2. Boot Flow with TBYB

### 2.1 FLASH_UPDATE Reboot

When firmware calls `reboot(REBOOT_TYPE_FLASH_UPDATE, delay_ms, flash_addr, 0)`:

1. Bootrom stores `flash_addr` as `reboot_params[0]` in watchdog scratch registers
2. Watchdog fires after `delay_ms`, resetting the chip
3. On next boot, bootrom reads `boot_type = BOOT_TYPE_FLASH_UPDATE` from watchdog scratch
4. Sets `flash_update_boot_offset = reboot_params[0] - XIP_BASE`

### 2.2 A/B Partition Selection with TBYB

In `s_varm_crit_choose_by_tbyb_flash_update_boot_and_version` (`varm_flash_boot.c:216`):

The bootrom compares two slots (X=A, Y=B). The decision logic:

1. **If X is the flash_update target:** X is preferred regardless of version. If Y has a
   higher version, Y is marked for erase (version downgrade support).

2. **If X is TBYB-flagged (but not the flash_update target):** Y is preferred.
   TBYB-flagged images are **always skipped on normal boot**.

3. **If Y is the flash_update target:** Y is preferred.

4. **If Y is TBYB-flagged (but not flash_update target):** Y is skipped, X is preferred.

5. **Neither is flash_update/TBYB:** Higher version wins, verified first.

**Key rule: TBYB-flagged images are ONLY bootable during FLASH_UPDATE boots where
the image's partition matches the flash_update target.**

### 2.3 Image Launch with TBYB

In `s_varm_crit_ram_trash_verify_and_launch_flash_image` (`varm_launch_image.c:208-222`):

```c
if (ctx->flash_update_boot_offset != parsed_block_loop->flash_start_offset) {
    // This image is NOT the flash_update target
    if (image_def->core.tbyb_flagged) {
        printf("NOT booting TBYB flagged image which isn't the flash update\n");
        rc = BOOTROM_ERROR_INVALID_STATE;
        goto verify_and_launch_flash_image_done;
    }
} else if (image_def->core.tbyb_flagged) {
    // This image IS the flash_update target AND is TBYB
    always->zero_init.tbyb_flag_flash_addr =
        image_def->core.enclosing_window.base +
        image_def->core.window_rel_block_offset + 4;
    printf("SAVING TBYB flash address %08x\n", always->zero_init.tbyb_flag_flash_addr);
}
```

### 2.4 TBYB Watchdog Setup

When a TBYB image is actually launched (`varm_launch_image.c:446-476`):

```c
if (!image_def->core.tbyb_flagged) {
    // Non-TBYB: implicit buy (erase other partition)
    s_varm_crit_buy_erase_other_version(false);
} else {
    // TBYB: set up watchdog for rollback
    printf("Setting %d second watchdog timer for TBYB\n", WATCHDOG_CTRL_TIME_BITS / 1000);
    s_varm_api_reboot(REBOOT2_FLAG_REBOOT_TYPE_NORMAL, WATCHDOG_CTRL_TIME_BITS, 0, 0);
}
```

The watchdog is armed with `WATCHDOG_CTRL_TIME_BITS = 0xFFFFFF` ticks (~16.7 seconds).
If `explicit_buy()` is not called within this window, the watchdog fires a **NORMAL reboot**
(not FLASH_UPDATE), so the TBYB image will be skipped on the next boot.


## 3. chain_image and TBYB — THE CRITICAL INTERACTION

`chain_image` (`varm_apis.c:529-583`) creates a new boot scan context:

```c
ctx->flash_update_boot_offset = (-window_base) - XIP_BASE;
if ((int32_t)window_base < 0) {
    window_base = -window_base;
}
```

When called with a **positive** `window_base` (which is the normal case from our bootloader):
- `flash_update_boot_offset = (-positive_value) - XIP_BASE` → a very large/negative value
- This value will **never match** any partition's flash offset
- `window_base` stays positive (the `if` branch is not taken)

Then `chain_image` calls `s_varm_crit_ram_trash_checked_ram_or_flash_window_launch`,
which eventually reaches `s_varm_crit_ram_trash_verify_and_launch_flash_image`.

**If the app's IMAGE_DEF has TBYB set:**
- `ctx->flash_update_boot_offset` (garbage) ≠ `parsed_block_loop->flash_start_offset` (app partition)
- `image_def->core.tbyb_flagged` is TRUE
- → **"NOT booting TBYB flagged image which isn't the flash update"**
- → Returns `BOOTROM_ERROR_INVALID_STATE`
- → **chain_image FAILS**

**To pass a flash_update context through chain_image:** The caller must negate `window_base`:
```c
chain_image(workarea, size, -region_base, region_size);
```
When `window_base` is negative:
- `flash_update_boot_offset = (-(-region_base)) - XIP_BASE = region_base - XIP_BASE` → correct partition offset
- `window_base = -window_base = region_base` → correct window base


## 4. The "Buy" (Commit) Mechanism

### explicit_buy() (`varm_launch_image.c:147-188`)

1. Disables the watchdog timer
2. Updates OTP rollback version if applicable (may cause reboot!)
3. If `tbyb_flag_flash_addr` is set:
   - Reads the 4KB sector containing the TBYB flag
   - Erases the sector
   - Clears the TBYB bit (bit 31 of word at offset +4 from block start marker)
   - Rewrites the sector
   - This makes the image bootable on normal boots
4. If `version_downgrade_erase_flash_addr` is set (i.e., the update is a lower version
   than the existing slot), erases that sector to prevent rollback to the higher-versioned
   old image. This address is only populated during A/B selection when a version downgrade
   is detected; for same-or-higher-version updates, nothing is erased.

**Buffer requirement:** At least 4KB for the sector read-modify-write.

From the bootrom (`varm_launch_image.c:163-165`):
```c
if (buffer_size < sector_size) {
    rc = BOOTROM_ERROR_BUFFER_TOO_SMALL;
    goto explicit_buy_done;
}
```

`sector_size = FLASH_SECTOR_SIZE = 4096`. The buffer **must** be at least 4096 bytes.

**Return value:** `explicit_buy()` returns `BOOTROM_OK` (0) on success AND when there is
nothing to do (no pending TBYB, no pending OTP version). There is no distinct "not a TBYB
boot" return code — the caller cannot distinguish "buy performed" from "nothing to buy"
by return value alone.


## 5. Rollback Behavior

- **Power cycle** after TBYB boot: Normal reboot, no FLASH_UPDATE context → TBYB image
  is skipped → old firmware boots
- **Watchdog timeout** (~16.7s): Same as power cycle — NORMAL reboot → old firmware
- **explicit_buy called**: Clears TBYB flag → image becomes permanent, other slot erased


## 6. Potential Failure Modes

### 6.1 TBYB on chain_image target (CONFIRMED BUG)
If the APP IMAGE_DEF has TBYB set and is loaded via chain_image with a positive
window_base, the bootrom will refuse to boot it. chain_image does NOT carry the
FLASH_UPDATE context unless the caller negates the window_base.

### 6.2 explicit_buy buffer too small
Our code uses 256 bytes. The bootrom requires 4096 bytes (one flash sector) to
perform the read-modify-write of the TBYB flag.

### 6.3 pick_ab_partition during TBYB
The pico SDK warns that calling `pick_ab_partition` before `explicit_buy` can clear
the saved flash erase address needed for version downgrade. The SDK provides
`rom_pick_ab_partition_during_update` as a wrapper. Our bootloader calls
`pick_ab_partition` directly.


## References

- `reference/pico-bootrom-rp2350/src/main/arm/varm_flash_boot.c` — A/B selection with TBYB
- `reference/pico-bootrom-rp2350/src/main/arm/varm_launch_image.c` — Image launch, TBYB watchdog, explicit_buy
- `reference/pico-bootrom-rp2350/src/main/arm/varm_blocks.c` — Block parsing, TBYB bit stripping
- `reference/pico-bootrom-rp2350/src/main/arm/varm_apis.c` — chain_image, reboot, pick_ab_partition
- `reference/pico-bootrom-rp2350/src/main/native/bootrom.h` — TBYB flag definitions
- `reference/pico-sdk/src/rp2_common/pico_bootrom/bootrom.c` — pick_ab_partition_during_update
