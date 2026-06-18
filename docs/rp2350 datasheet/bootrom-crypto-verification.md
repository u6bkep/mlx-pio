# RP2350 Bootrom Cryptographic Verification Reference

This document describes how the RP2350 bootrom performs cryptographic verification of images during boot. Based on analysis of the bootrom source code in `reference/pico-bootrom-rp2350/`.

## Overview

The RP2350 bootrom uses a combination of SHA-256 hashing and secp256k1 ECDSA signatures to verify the integrity and authenticity of boot images. Verification is performed in a hardened manner with extensive redundancy checks to resist fault injection attacks.

## Key Source Files

| File | Purpose |
|------|---------|
| `varm_blocks.c` | Block parsing, hash computation, hash/signature verification |
| `varm_launch_image.c` | Image launch logic, security checks before execution |
| `varm_flash_boot.c` | Flash boot orchestration, partition scanning |
| `arm8_sig.c` | ECDSA signature verification wrapper |
| `arm8_sig.h` | Signature types and function declarations |

---

## Block Item Types (from picobin.h)

These are the cryptography-related item types found in PICOBIN blocks:

| Item Type | Value | Purpose |
|-----------|-------|---------|
| `PICOBIN_BLOCK_ITEM_1BS_HASH_DEF` | `0x47` | Defines what hash algorithm is used and how many block words are hashed |
| `PICOBIN_BLOCK_ITEM_SIGNATURE` | `0x09` | Contains public key (64 bytes) and signature (64 bytes) |
| `PICOBIN_BLOCK_ITEM_HASH_VALUE` | `0x4B` | Contains the expected hash value (for hash-only verification) |
| `PICOBIN_BLOCK_ITEM_SALT` | `0x0C` | Salt value for EXEC2 feature (optional) |

### Hash Types

| Hash Type | Value |
|-----------|-------|
| `PICOBIN_HASH_SHA256` | `0x01` |

### Signature Types

| Signature Type | Value |
|----------------|-------|
| `PICOBIN_SIGNATURE_SECP256K1` | `0x01` |

---

## Hash Computation Process

The bootrom computes a SHA-256 hash over:

1. **LOAD_MAP regions** - The actual binary content referenced by the LOAD_MAP
2. **Block words** - The first N words of the block itself (as specified by `hash_def_block_words_included`)

### Hash Computation Flow (varm_blocks.c:842-1107)

```
1. Initialize SHA-256 state: sb_sha256_init(&sha)

2. For each LOAD_MAP entry:
   a. Validate addresses (storage and runtime)
   b. If booting: Copy data from flash to RAM per LOAD_MAP
   c. Hash the data at the destination address:
      sb_sha256_update_32(&sha, src, size)

3. Hash the block words:
   sb_sha256_update_32(&sha, block_data, hash_def_block_words_included * 4)

4. Finalize the hash:
   sb_sha256_finish(&sha, signature_workspace->hash.bytes)
```

### Key Points About Hashing

- The hash covers **LOAD_MAP content first**, then **block words**
- Block words hashed = first N words of the block (NOT including HASH_VALUE or SIGNATURE items)
- The `hash_def_block_words_included` field specifies exactly how many block words to include
- Zero-fill entries in LOAD_MAP (storage_address = 0) contribute their size to the hash

### LOAD_MAP Address Translation

For LOAD_MAP entries, the bootrom translates physical addresses to storage addresses:

```c
// From varm_blocks.c:907-920
uint32_t lma_to_storage = parsed_block->enclosing_window.base 
                        + parsed_block->slot_roll 
                        - XIP_BASE 
                        + ((const parsed_image_def_t *)parsed_block)->rolling_window_delta;

// For flash addresses (not SRAM/XIP-RAM):
from_storage_addr = map_storage_address_value + lma_to_storage;
```

This means LOAD_MAP uses **physical addresses** (e.g., `0x10000200`) which the bootrom translates to actual flash locations based on the partition.

---

## Signature Verification Process

### Signature Block Structure

The SIGNATURE item (type `0x09`) has the following structure:
- **Size**: 33 words (1 header + 32 data = 132 bytes)
- **Layout**:
  - Header word: size=33, type=0x09, sig_type in upper byte
  - Words 1-16 (64 bytes): Public key (secp256k1)
  - Words 17-32 (64 bytes): Signature (ECDSA)

### Key Fingerprint Verification (varm_blocks.c:221-286)

Before verifying a signature, the bootrom checks if the public key matches an OTP-stored key fingerprint:

```c
// Pseudocode for key matching
1. Hash the public key: SHA256(public_key) -> digest
2. Read valid key flags from OTP_DATA_BOOT_FLAGS1_ROW
3. For each of 4 possible bootkeys (BOOTKEY0-3):
   a. Skip if key is marked invalid or not valid
   b. Compare digest against OTP_DATA_BOOTKEYn_* rows (16 halfwords = 32 bytes)
   c. If match found: return hx_key_match_true()
4. If no match: return hx_key_match_false()
```

### Signature Verification (arm8_sig.c)

The actual ECDSA verification uses the "sweet-b" library:

```c
hx_xbool s_arm8_verify_signature_secp256k1(
    uint32_t context_buffer[SIG_CONTEXT_SIZE/4],  // 0x200 bytes workspace
    const sb_sw_public_t public_key[1],            // 64 bytes
    const sb_sw_message_digest_t digest[1],        // 32 bytes (SHA-256 hash)
    const sb_sw_signature_t signature[1]           // 64 bytes
) {
    sb_sw_context_t *context = (sb_sw_context_t *)context_buffer;
    sb_verify_result_t res = sb_sw_verify_signature(
        context,
        signature,
        public_key,
        digest,
        NULL,
        SB_SW_CURVE_SECP256K1
    );
    return res;  // XORed with HX_XOR_SIG_VERIFIED for hardening
}
```

### Signature Anti-Replay Protection

The bootrom modifies the signature in RAM during parse to prevent replay attacks:

```c
// During parse (varm_blocks.c:423):
block_data[word_index+16] += rcp_canary_get_nodelay(CTAG_SIG_DISRUPTER);

// Before verification (varm_blocks.c:1155):
((sb_sw_signature_t *)signature)->words[0] -= rcp_canary_get_nodelay(CTAG_SIG_DISRUPTER);
```

This ensures the signature was actually parsed by the bootrom and not injected from a canned buffer.

---

## Verification Decision Logic

### The `s_varm_crit_ram_trash_verify_block` Function (varm_blocks.c:836-1240)

This is the main verification function. It decides what verification is needed based on:

1. **`sig_required`** - Whether a signature is required (set by OTP secure boot flag)
2. **`hash_required`** - Whether a hash is required (set by OTP flags)
3. **Block contents** - Whether HASH_VALUE and/or SIGNATURE items are present

### Verification Outcomes

| Condition | Action | Result |
|-----------|--------|--------|
| `sig_required=true` + valid signature + key matches OTP | Verify signature | `verified = sig_verified && key_match` |
| `sig_required=true` + no signature or wrong key | Fail | `verified = false` |
| `sig_required=false` + HASH_VALUE present | Compare hash | `verified = hash_matches` |
| `sig_required=false` + no hash | Skip hash | `verified = !sig_required && !hash_required` |

### Code Flow (simplified)

```c
if (sig_required) {
    if (sig_otp_key_match && hash_def_present) {
        // Verify signature
        sig_matches = s_arm8_verify_signature_secp256k1(...);
        verified = sig_matches && sig_otp_key_match;
    } else {
        // Missing signature or wrong key
        goto verify_fail;
    }
} else if (do_hash) {
    // Compare computed hash against HASH_VALUE in block
    for (i = 0; i < hash_value_word_count; i++) {
        if (computed_hash[i] != block_data[hash_value_word_index + i]) {
            goto verify_fail;
        }
    }
    verified = !sig_required;
} else {
    // No signature or hash required
    verified = !sig_required && !hash_required;
}
```

---

## Launch-Time Security Checks

Before launching an image, additional security checks are performed in `s_varm_crit_ram_trash_verify_and_launch_image` (varm_launch_image.c:264-291):

```c
// 1. Image must be verified
if (hx_is_false(is_image_def_verified(image_def))) {
    return BOOTROM_ERROR_NOT_PERMITTED;
}

// 2. In secure mode, signature key must match OTP
hx_assert_notx_orx_true(always->secure, image_def->sig_otp_key_match_and_block_hashed);

// 3. If secure boot enabled, signature must be verified
hx_assert_bequal(
    hx_xbool_to_bool(always->secure),
    hx_xbool_to_bool(is_image_def_signature_verifiedx(image_def))
);

// 4. Rollback version check (if required)
hx_assert_or(has_rollback_version, !rollback_required);
```

---

## Hardening Features

The bootrom uses extensive hardening against fault injection:

### 1. Hardened Booleans (hx_bool, hx_xbool)

Instead of simple true/false, the bootrom uses XOR-masked values:
- `hx_true()` = `0xa500a500` (bit pattern)
- `hx_false()` = `0xc300c300`
- Values are XORed with known patterns and checked for consistency

### 2. Step Counting

The bootrom tracks execution steps to detect skipped code:
```c
hx_set_step(STEPTAG_X);
// ... code ...
hx_check_step(STEPTAG_X + expected_increments);
```

### 3. Canary Values

Function entry/exit is protected:
```c
canary_entry(FUNCTION_TAG);
// ... function body ...
canary_exit_return(FUNCTION_TAG, result);
```

### 4. Redundant Checks

Critical comparisons are performed multiple times with different registers:
```c
hx_assert_equal2i(value_a, value_b);  // Checks value_a == value_b twice
```

---

## OTP Configuration Bits

The following OTP bits control cryptographic verification:

| OTP Location | Purpose |
|--------------|---------|
| `OTP_DATA_BOOT_FLAGS0_SECURE_BOOT_ENABLE_LSB` | Enable secure boot (require signatures) |
| `OTP_DATA_BOOT_FLAGS0_ROLLBACK_REQUIRED_LSB` | Require rollback version in images |
| `OTP_DATA_BOOT_FLAGS1_KEY_VALID_*` | Which boot keys (0-3) are valid |
| `OTP_DATA_BOOT_FLAGS1_KEY_INVALID_*` | Which boot keys are invalidated |
| `OTP_DATA_BOOTKEY0_*` through `OTP_DATA_BOOTKEY3_*` | SHA-256 fingerprints of allowed public keys |

---

## Summary: What Gets Hashed

For a signed bootloader with LOAD_MAP, the hash includes:

1. **Binary content** at the locations specified by LOAD_MAP entries
   - For packaged binaries: hash of data after it's "copied" to runtime address
   - Storage addresses are translated from physical to partition-relative
   
2. **Block words** from the start of the block up to `hash_def_block_words_included`
   - This typically includes: IMAGE_TYPE, VERSION, LOAD_MAP, HASH_DEF
   - Excludes: SIGNATURE, HASH_VALUE (these come after the hashed portion)

### Block Layout for Signing

```
Block start (offset 0):
├── BLOCK_MARKER_START (not counted in block_words)
├── [Word 1] IMAGE_TYPE item
├── [Word 2] VERSION item (optional)
├── [Word 3-N] LOAD_MAP item
├── [Word N+1] HASH_DEF item (hash_def_block_words_included points here-ish)
│   ↑ Hash covers up to here ↑
├── [Word N+2] SIGNATURE item (33 words) ← NOT hashed
├── [Word ...] HASH_VALUE item (optional, for hash-only verification)
├── [Word X] LAST item
├── next_block_offset
└── BLOCK_MARKER_END
```

---

## Troubleshooting Hash/Signature Issues

### Common Failure Modes

1. **"no hash-def/signature found"** - Block doesn't have required HASH_DEF or SIGNATURE items
2. **"wrong signature key"** - Public key in SIGNATURE doesn't match any OTP bootkey
3. **"HASH mismatch"** - Computed hash doesn't match HASH_VALUE
4. **Signature verification fails** - ECDSA verification returns false

### Debugging Steps

1. Check OTP secure boot flags are set correctly
2. Verify BOOTKEY fingerprints in OTP match your signing key's SHA-256 hash
3. Ensure `hash_def_block_words_included` covers exactly the right content
4. Verify LOAD_MAP addresses use physical addressing (not partition-relative)
5. Check that binary content at LOAD_MAP addresses is correct

### Key Insight: Hash Order

The hash is computed as:
```
SHA256(LOAD_MAP_content_0 || LOAD_MAP_content_1 || ... || first_N_block_words)
```

NOT:
```
SHA256(block_words || LOAD_MAP_content)  // WRONG ORDER
```

This ordering is critical for generating valid signatures.
