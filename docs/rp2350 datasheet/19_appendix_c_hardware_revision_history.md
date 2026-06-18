# Appendix C: Hardware revision

# history

## This appendix summarises the differences between RP2350 hardware revisions, referred to as steppings. To determine

## the stepping of an unknown device, check the package markings, as described in Section 14.4. Software running on the

## device can also read the CHIP_ID.REVISION register field, or call the rp2350_chip_version() SDK function.

## In this appendix:

## • The term^ fix^ refers to an issue that’s fully resolved.

## • The term^ mitigate^ refers to an issue that’s either partially resolved or believed to be fully resolved but with an

## unpredictable underlying cause, such as a fault injection vulnerability.

## • The term^ update^ refers to any other difference between steppings.

## This appendix offers a high-level overview; for detailed information, refer to individual errata entries in appendix E.

## RP2350 A

## Stepping A2 is identified by a CHIP_ID.REVISION value of 0x2.

## A2 is the first generally available version of RP2350, and the earliest stepping documented in this datasheet.

## RP2350 A

## Stepping A3 is identified by a CHIP_ID.REVISION value of 0x3.

## Hardware changes

## Stepping A3 introduces the following hardware fixes and mitigations:

## • Fix RP2350-E3: in QFN-60 package,^ GPIO_NSMASK^ controls wrong^ PADS^ registers. Hardware now remaps^ GPIO_NSMASK^ to

## the correct PADS registers in the QFN60 package.

## • Fix RP2350-E9: increased leakage current on Bank 0 GPIO when pad input is enabled. The pad circuit is modified

## to eliminate the erroneous leakage path through the input buffer.

## • Mitigate RP2350-E12: inadequate synchronisation of USB status signals. Signals used by the bootrom are now

## valid across the full PVT range in the bootrom’s clock configuration. Other software must not rely on these

## mitigations.

## • Mitigate RP2350-E16:^ USB_OTP_VDD^ disruption can cause corrupt OTP row read data. The following changes apply:

## ◦ Multiple changes to the OTP PSM and OTP read circuits to detect unreliable operation.

## ◦ RISC-V debug is now disabled by CRIT1.SECURE_DEBUG_DISABLE, in addition to CRIT1.DEBUG_DISABLE. (On

## A2, only the latter bit was used.)

## ◦ CRIT0.ARM_DISABLE no longer disables the Arm processors.

## ◦ Programming both CRIT0.ARM_DISABLE and CRIT0.RISCV_DISABLE is decoded as an illegal combination,

## and the device won’t boot.

- Update^ the reset state of the following clock configuration registers:

### ◦ ROSC: FREQA.DS0_RANDOM and FREQA.DS1_RANDOM from^0 to^1 , enabling randomisation of first two drive

##### stages.

### ◦ CLOCKS: CLK_SYS_CTRL.SRC from^0 to^1 (select AUX source).

### ◦ CLOCKS: CLK_SYS_CTRL.AUXSRC from^0 to^2 (select ROSC as AUX source).

#### Bootrom changes

##### The A3 bootrom introduces the following changes:

- Fix RP2350-E10: UF2 drag-and-drop doesn’t work with partition tables. This previously required a workaround in

##### picotool, but the A3 bootrom no longer requires this workaround. picotool retains the workaround for compatibility

##### with A2.

- Fix RP2350-E13: a binary containing an explicitly invalid^ IMAGE_DEF^ followed by a valid^ IMAGE_DEF^ (in that order) fails

##### to boot.

- Fix RP2350-E14: the bootrom connect_internal_flash() function always uses pin 0, ignoring any configured

##### FLASH_DEVINFO CS1 chip select pin.

- Fix RP2350-E15: the bootrom otp_access() function applies incorrect access permission to pages 62 and 63.
- Fix RP2350-E19: RP2350 reboot halts if certain bits are set in FRCE_OFF when rebooting.
- Mitigate RP2350-E20: an attacker with physical access to the chip and the ability to physically glitch the CPU at

##### precise times could cause unsigned code execution on a secured RP2350 by targeting legitimate Non-secure calls

##### to the bootrom reboot() function.

- Mitigate RP2350-E21: an attacker with physical access to the chip and the ability to physically glitch the CPU at

##### precise times, could extract sensitive data from OTP on a RP2350 in BOOTSEL mode.

- Fix RP2350-E22: parsing a malformed lollipop block loop causes the system to halt rather than fail.
- Fix RP2350-E23: PICOBOOT GET_INFO command always returns zero for PACKAGE_SEL)
- Mitigate RP2350-E26: RCP random delays can create a side-channel. These delays are disabled in the bootrom.
- Update^ the early boot path to change the^ clk_ref^ divider from^1 to^5 , and the ROSC divider from^8 to^2.

### ◦ Together with the register reset state changes, this increases the boot^ clk_sys^ frequency by a factor of 4, to

##### approximately 48 MHz.

### ◦ These changes reduce boot time and fault injection susceptibility.

### ◦ These changes apply for all boot outcomes, including watchdog and POWMAN vector boot.

## RP2350 A

##### Stepping A4 is identified by a CHIP_ID.REVISION value of 0x8.

#### Hardware Changes

##### This stepping has no hardware changes.

#### Bootrom Changes

##### The A4 bootrom introduces the following changes:

- Fix RP2350-E18: the RP2350 forever fails to boot if FLASH_PARTITION_SLOT_SIZE contains an invalid ECC bit

##### pattern. This issue is a consequence of RP2350-E17 (a guarded read on a single ECC OTP row causes a fault if the

##### data in the adjacent row isn’t also valid ECC data). The underlying hardware issue isn’t resolved, but the bootrom

##### avoids the issue in this instance.

- Mitigate RP2350-E24: an attacker with physical access to the chip, moderate hardware, and the ability to

##### physically glitch the CPU at precise times, could cause unsigned code execution on a secured RP2350. The A

##### bootrom contains additional fault injection mitigations for this vulnerability, and for other potential vulnerabilities

##### with the same underlying mechanism.

- Fix RP2350-E25: a LOAD_MAP that uses non-word sizes previously didn’t cause an error. The bootrom now

##### correctly rejects these structures.

