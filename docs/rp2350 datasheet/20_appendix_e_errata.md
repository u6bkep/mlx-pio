# Appendix E: Errata

## Alphabetical by section.

## ACCESSCTRL

## RP2350-E

Reference (^) RP2350-E
Summary (^) In QFN-60 package, GPIO_NSMASK controls wrong PADS registers
Affects (^) RP2350 A2, QFN-60 package only
Description (^) RP2350 remaps IOs, their control registers and their ADC channels so that both package sizes appear to

## have consecutively numbered GPIOs, even though for physical design reasons the QFN-60 package

## bonds out a sparse selection of IO pads.

## The connection between the GPIO_NSMASK0/GPIO_NSMASK1 registers and the PADS registers doesn’t

## take this remapping into account. Consequently, in the QFN-60 package only, the GPIO_NSMASK0 register bits

## are applied to registers for the wrong pads. Specifically, PADS_BANK0 registers 29 through 0 are controlled

## by the concatenation of GPIO_NSMASK bits 47 through 44, 39 through 33, 30 through 28, 24 through 17 and

## 15 through 8 (all inclusive ranges).

## This means that granting Non-secure access to the PADS registers in the QFN-60 package doesn’t allow

## Non-secure software to control the correct pads. It may also allow Non-secure control of pads that aren’t

## granted in GPIO_NSMASK0.

## The QSPI PADS registers (Bank 1) aren’t affected because these aren’t remapped for different packages.

Workaround (^) Disable Non-secure access to the PADS registers by clearing PADS_BANK0.NSP, NSU.

## Implement a Secure Gateway (Arm) or ecall handler (RISC-V) to permit Non-Secure/U-mode code to

## read/write its assigned PADS_BANK0 registers.

Fixed by (^) RP2350 A3, Documentation, Software

## Bootrom

## RP2350-E

Reference (^) RP2350-E
Summary (^) UF2 drag-and-drop doesn’t work with partition tables
Affects (^) RP2350 A
Description (^) When dragging and dropping a UF2 onto the USB Mass Storage Device, the bootrom on chip revision A

## doesn’t set up the flash before checking the partition table. This causes the UF2 download to fail if there

## is a partition table present.

## ACCESSCTRL 1357

Workaround (^) Add a single block at the start of the UF2 with an Absolute family ID, targeting the end of Flash, with block

#### number set to 0 and number of blocks set to 2. This block is written to flash first but doesn’t reboot the

#### device, and sets up the flash for the rest of the UF2 to be downloaded correctly.

#### This is handled for you automatically by picotool in the SDK, which adds this block when generating UF2s

#### if the --abs-block flag is specified.

#### This workaround means that the last block of flash is erased when downloading such a UF2, which could

#### overwrite user data.

#### As of picotool version 2.1.0, this additional UF2 block is marked with a Raspberry Pi specific UF

#### extension UF2_EXTENSION_RP2_IGNORE_BLOCK (0x9957e304). The RP2350 A3 and later bootroms, contain a fix for

#### this erratum, and therefore don’t need the workaround. The presence of this extension in the UF2 block

#### allows the newer RP2350 to recognize and ignore the workaround block, thus avoiding the risk of

#### overwriting user data.

Fixed by (^) RP2350 A3 bootrom, Documentation, Software

### RP2350-E

Reference (^) RP2350-E
Summary (^) A binary containing an explicitly invalid IMAGE_DEF followed by a valid IMAGE_DEF (in that order) fails to boot
Affects (^) RP2350 A
Description (^) When the block loop of a binary contains an IMAGE_DEF that is explicitly invalid before the valid IMAGE_DEF for

#### RP2350, booting from that binary fails.

#### An IMAGE_DEF is explicitly invalid if either:

- It is for RP
- It doesn’t have a rollback version, and the BOOT_FLAGS0.ROLLBACK_REQUIRED flag is set in OTP

Workaround (^) Instead of an explicitly invalid IMAGE_DEF, use an IGNORED item. RP2040 doesn’t require an IMAGE_DEF to boot a

#### binary, and when using rollback, the invalid IMAGE_DEF is ignored anyway.

#### SDK uses this workaround for RP2040 binaries. When you set PICO_CRT0_INCLUDE_PICOBIN_BLOCK, the SDK

#### uses an IGNORED item instead of an IMAGE_DEF for RP2040 binaries. You can override this behaviour and use

#### an IMAGE_DEF by setting PICO_CRT0_INCLUDE_PICOBIN_IMAGE_TYPE_ITEM. For an additional example, see the

#### universal binaries in pico-examples.

#### picotool uses this workaround for rollback versions. When you use picotool seal to seal a binary and add

#### a rollback version, it converts the first IMAGE_DEF without a rollback version to an IGNORED item.

Fixed by (^) RP2350 A3 bootrom, Documentation, Software

### RP2350-E

Reference (^) RP2350-E
Summary (^) The bootrom connect_internal_flash() function always uses pin 0, ignoring any configured

#### FLASH_DEVINFO CS1 chip select pin

Affects (^) RP2350 A

#### Bootrom 1358

Description (^) When using the bootrom function connect_internal_flash() to configure CS1 (for instance, during a flash

#### boot), the bootrom always configures the pad registers for pin 0, ignoring any CS1 pin specified in

#### FLASH_DEVINFO.

#### As a result, the specified CS1 pin remains isolated (see Section 9.7). Accesses to the QSPI device

#### connected to the second chip select fails unless CS1 is connected to pin 0.

#### FLASH_DEVINFO can be configured in OTP or at runtime. For more information, see flash_devinfo16_ptr.

Workaround (^) Manually configure the CS1 pads registers to remove the isolation after using the bootrom

#### connect_internal_flash() function. Alternatively, connect CS1 to pin 0.

Fixed by (^) RP2350 A3 bootrom, Documentation, Software

### RP2350-E

Reference (^) RP2350-E
Summary (^) The bootrom otp_access() function applies incorrect access permission to pages 62 & 63
Affects (^) RP2350 A
Description (^) The bootrom otp_access() function incorrectly applies the access permissions specified in OTP rows

#### PAGE62_LOCK1 and PAGE63_LOCK1 to the entirety of their respective OTP pages (62 and 63). This is

#### incorrect, as pages 62 and 63 contain lock words for other pages: each lock word is instead protected by

#### the permissions of the corresponding page.

#### The ATE programming then locks down write access from non-Secure software and the bootloader to the

#### page 63 lock word (to prevent non-Secure setting of the RMA flag), and write access from non-Secure

#### software to the page 62 lock word. This prevents non-Secure software from modifying any of the OTP

#### page locks, and the bootloader from modifying the locks for pages 32-63.

Workaround (^) When running code on the device, don’t use the non-Secure otp_access() function to set locks for OTP

#### pages. To set OTP page locks from non-Secure code, implement your own Secure API to do this that can

#### be called from non-Secure code.

#### Page locks for OTP pages 32-63 can be set by picotool using the picotool otp permissions command. This

#### command loads a Secure binary into XIP SRAM on the device to change the permissions before

#### rebooting back into the USB bootloader.

Fixed by (^) RP2350 A3 bootrom, Documentation, Software

### RP2350-E

Reference (^) RP2350-E
Summary (^) The RP2350 will forever fail to boot if FLASH_PARTITION_SLOT_SIZE contains an invalid ECC bit pattern
Affects (^) RP2350 A2, RP2350 A

#### Bootrom 1359

Description (^) If ECC row programming is interrupted, an ECC row may contain a value that fails ECC validation.

#### Because any ECC could potentially contain an invalid, partially written value, the bootrom uses a separate

#### "enable" flag in OTP to indicate whether a particular ECC row is expected to contain a valid value. The

#### user is expected to only set this flag after a particular ECC row is known to have been written correctly.

#### For FLASH_PARTITION_SLOT_SIZE, the "enable" flag is

#### BOOT_FLAGS0.OVERRIDE_FLASH_PARTITION_SLOT_SIZE.

#### In the case of FLASH_PARTITION_SLOT_SIZE, the bootrom reads the row value and asserts the value is

#### valid before checking the enable flag, and thus the boot process will hang if the

#### BOOT_FLAGS0.OVERRIDE_FLASH_PARTITION_SLOT_SIZE row in OTP contains an invalid ECC value.

Workaround (^) Don’t program FLASH_PARTITION_SLOT_SIZE or at least be aware that doing so may brick your device if

#### the programming operation is interrupted or fails.

Fixed by (^) RP2350 A4 bootrom, Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) RP2350 reboot hangs if certain bits are set in FRCE_OFF when rebooting.
Affects (^) RP2350 A
Description (^) An incorrect assertion in the boot path, assumes the all bits (other than FRCE_OFF.PROC1) are clear.

#### These bits can only be set during boot if the user had set them and then re-entered the boot path.

Workaround (^) Don’t perform a WATCHDOG or POWMAN boot, or a core0 reset with bits other than FRCE_OFF.PROC1 set.
Fixed by (^) RP2350 A3 bootrom, Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) An attacker with physical access to the chip and the ability to physically "glitch" the CPU at precise times,

#### could cause unsigned code execution on a secured RP2350 by targeting legitimate Non-secure calls to

#### the bootrom reboot() function

Affects (^) RP2350 A
Description (^) The RP2350 bootrom provides a reboot() function to reboot the RP2350. This method is potentially

#### accessible to Non-secure callers via the PICOBOOT interface (e.g. picotool) or, if the corresponding

#### permission is set, to Non-secure code running on the device.

#### A particular reboot type (REBOOT_TYPE_PC_SP) isn’t allowed in the bootrom reboot() function when called

#### from Non-secure code as it launches user-provided code in a Secure state post reboot. The reboot()

#### function correctly disallows this reboot type when called from a Non-secure context. However, if a valid

#### reboot type (e.g. REBOOT_TYPE_NORMAL) is passed to the function instead, a late, precisely-timed processor

#### glitch can cause an incorrect code path to be taken, which configures the WATCHDOG scratch registers

#### in a way that allows secure execution of user-provided code post reboot.

#### Bootrom 1360

Workaround (^) If any WATCHDOG based reboot types other than into the regular boot path aren’t required (this includes

#### programmatic reboots into BOOTSEL mode and FLASH_UPDATE boots which are important when using

#### A/B partitions), the OTP flag BOOT_FLAGS0.DISABLE_WATCHDOG_SCRATCH can be set, which causes

#### the WATCHDOG scratch registers to be completely ignored during boot, meaning that the only type of

#### boot available via WATCHDOG reset is regular boot.

#### A more refined approach would be to disable use of the reboot() function from Non-secure code. This is

#### the default case for Non-secure code started by a secure application (see Section 5.4.2). However, the

#### BOOTSEL mode bootloader is itself a Non-secure application that does have access to the function.

#### BOOTSEL mode, however, can be disabled if not needed through

#### BOOT_FLAGS0.DISABLE_BOOTSEL_UART_BOOT,

#### BOOT_FLAGS0.DISABLE_BOOTSEL_USB_PICOBOOT_IFC, and

#### BOOT_FLAGS0.DISABLE_BOOTSEL_USB_MSD_IFC.

#### BOOT_FLAGS0.DISABLE_BOOTSEL_USB_PICOBOOT_IFC is the most important because PICOBOOT provides

#### a conduit for a user to pass specific parameters to the bootrom reboot() function. However, any other use

#### of BOOTSEL mode could be vulnerable in conjunction with some other future attack on the Non-secure

#### code.

Credit (^) Marius Muench
Fixed by (^) RP2350 A3 bootrom, Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) An attacker with physical access to the chip, and the ability to physically "glitch" the CPU at precise times,

#### could potentially extract sensitive data from OTP on a RP2350 in BOOTSEL mode.

#### Affects RP2350 A

Description (^) An attacker with physical access to the chip, could precisely time physical glitch attacks during entry to

#### BOOTSEL mode and cause some OTP page access permissions for BOOTSEL mode not to be applied,

#### leading to possible exposure of sensitive data.

#### The RP2350 BOOTSEL mode exposes an API over PICOBOOT such that a user can read or write OTP

#### rows through picotool. Certain OTP rows (such as encryption keys or other secrets) shouldn’t be readable

#### through this method. Equally, certain rows might be protected against writes. This is handled at the page

#### (64 row) level by page locks stored in OTP.

#### On entry to BOOTSEL mode, the OTP should be locked down such that no software (including Secure

#### software) can access anything not marked as accessible to BOOTSEL mode. However, with two precisely

#### timed processor glitches, it’s possible to prevent a page being correctly locked.

Workaround (^) 1. Disable the BOOTSEL mode bootloader altogether via

#### BOOT_FLAGS0.DISABLE_BOOTSEL_UART_BOOT,

#### BOOT_FLAGS0.DISABLE_BOOTSEL_USB_PICOBOOT_IFC, and

#### BOOT_FLAGS0.DISABLE_BOOTSEL_USB_MSD_IFC.

#### BOOT_FLAGS0.DISABLE_BOOTSEL_USB_PICOBOOT_IFC is the most important, as PICOBOOT provides

#### the conduit for a user to access the OTP, however any other use of BOOTSEL mode could be vulnerable in

#### conjunction with some other future attack on the non-Secure code.

#### 1. Use an OTP access key (Section 13.5.2) to protect OTP data you don’t want accessed from

#### BOOTSEL mode, although this only helps if the data isn’t needed until your application can provide

#### the key.

Credit (^) Thomas Roth

#### Bootrom 1361

Fixed by (^) RP2350 A3 bootrom, Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) Parsing a malformed "lollipop" block loop will cause a hang rather than a failure
Affects (^) RP2350 A
Description (^) PARTITION_TABLEs and IMAGE_DEFs metadata are stored as part of a block loop. These block loops are parsed

#### during boot and at other times. A "lollipop" block loop is an invalid block loop, which loops back from the

#### last block to a block, which isn’t the first. Such an invalid block loop is never generated by the SDK or by

#### picotool; however, it could potentially be generated by other tooling.

Workaround (^) Don’t use "lollipop" block loops. If you program a "lollipop" block loop into flash such that it’s read during

#### the boot process, it will cause a boot hang and also a hang on entry into BOOTSEL mode. Therefore, to re-

#### enable booting, you must clear the flash in some other way, for example, from the debugger over SWD.

Fixed by (^) RP2350 A3 bootrom, Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) PICOBOOT GET_INFO command always returns zero for PACKAGE_SEL
Affects (^) RP2350 A
Description (^) The PICOBOOT GET_INFO command can be used to get system information in a similar way to the

#### bootrom get_sys_info() function. This information can include the value read from PACKAGE_SEL, which

#### indicates whether the RP2350 package is QFN60 or QFN80.

#### The SYSINFO block is erroneously left in reset when entering BOOTSEL mode, and thus this register is read

#### as zero when accessed via PICOBOOT.

#### This problem doesn’t affect use of the bootrom get_sys_info() function itself from user code.

Workaround (^) Determine the package size by reading register_link_macro:[register=OTP_DATA_NUM_GPIOS_ROW] via

#### the PICOBOOT OTP_READ command instead. This workaround is used by picotool.

Fixed by (^) RP2350 A3 bootrom, Documentation, Software

### RP2350-E

Reference (^) RP2350-E
Summary (^) An attacker with physical access to the chip, moderate hardware, and the ability to physically "glitch" the

#### CPU at precise times, could cause unsigned code execution on a secured RP2350.

Affects (^) RP2350 A2, RP2350 A

#### Bootrom 1362

Description (^) An attacker with physical access to the chip, and the ability to switch the contents of "flash" as read by

#### the RP2350 over QSPI during boot at precise times, could, combined with a precisely-timed physical

#### "glitch" attack of the CPU, trick the bootrom into checking the signature of data other than the program

#### binary as loaded into memory during secure boot.

#### If this "other" data passes the signature check, then the attacker’s binary is executed without itself having

#### passed a signature check, which allows the user to run arbitrary unsigned code on the RP2350.

Credit (^) Kévin Courdesses (see https://courk.cc/rp2350-challenge-laser#flash-memory-organization)
Workaround (^) None
Fixed by (^) RP2350 A4 bootrom

### RP2350-E

Reference (^) RP2350-E
Summary (^) A LOAD_MAP that uses non-word sizes doesn’t cause an error.
Affects (^) RP2350 A2, RP2350 A
Description (^) Non-word sizes in a LOAD_MAP aren’t supported and were documented as such. However, they don’t

#### currently cause an error. Whilst non-word sizes might currently work in some certain cases, you should

#### never use them because they might not work as you expect in all cases and can be properly treated as an

#### error in the future.

#### The SDK and picotool don’t generate such LOAD_MAPs with non-word sized entries.

Workaround (^) Don’t use non-word (not multiple of 4) sizes in a LOAD_MAP. A best practice is to make sure that linker

#### memory segments are both word-sized and word-aligned.

#### As of the RP2350 A4 bootrom, non-word sizes are detected when checking the LOAD_MAP and cause the

#### IMAGE_DEF to be considered invalid if present.

Fixed by (^) RP2350 A4 bootrom, Documentation
Bus Fabric

### RP2350-E

Reference (^) RP2350-E
Summary (^) Bus priority controls apply to wrong managers for APB and FASTPERI arbiters.
Affects (^) RP2350 A2, RP2350 A3, RP2350 A

#### Bus Fabric 1363

Description (^) RP2350 bus fabric consists mainly of an AHB5 crossbar, where 6 upstream ports (managers) are routed

#### to 15 downstream crossbar ports. Figure 5 shows the overall structure of the bus fabric, including this

#### crossbar. Because there can be multiple accesses to a given downstream crossbar port on any one cycle,

#### an arbiter circuit selects one transfer to forward to the downstream port, and stalls all other transfers

#### targeting this port.

#### The BUSCTRL BUS_PRIORITY register controls these arbiter circuits. It configures a 1-bit priority level for

#### each of the following four groups of AHB5 managers:

- DMA write
- DMA read
- Core 0 instruction fetch and core 0 load/store
- Core 1 instruction fetch and core 1 load/store

#### Accesses from high-priority managers are always routed preferentially over those from low-priority

#### managers. Multiple accesses from managers of the same priority are processed one at a time, taking

#### turns in a repeating cycle (round-robin arbitration).

#### On the FASTPERI and APB arbiters, these signals are mis-wired, such that the wrong managers are

#### prioritised:

- BUS_PRIORITY.PROC0 controls DMA write priority
- BUS_PRIORITY.PROC1 controls core 0 load/store priority
- BUS_PRIORITY.DMA_R controls core 1 load/store priority
- BUS_PRIORITY.DMA_W controls DMA read priority

#### The BUS_PRIORITY controls are applied correctly for all other arbiters: ROM, SRAM, and XIP.

#### For example, if the DMA_R and DMA_W bits were set, this would prioritise DMA over processor access to ROM,

#### SRAM, and XIP. However, peripheral access would prioritise DMA read and core 1 load/store over DMA

#### write and core 0 load/store.

Workaround (^) There is no complete fix, but the necessary prioritisation can often still be achieved by configuring

#### BUS_PRIORITY for correct priority at the peripherals, and then arranging buffers in SRAM to minimise

#### contention, such as using SRAM8 or SRAM9 as processor-private memories.

#### Also consider the following approaches:

- Split RAM access across the SRAM0 to SRAM3 and SRAM4 to SRAM7 striped regions to further

#### reduce RAM contention.

- Try to reduce overall peripheral bandwidth demand by using wider accesses for peripherals that

#### support it. For example, SPI supports 16-bit data, and HSTX and PIO support 32-bit data.

- Avoid processor polling of peripheral status registers. Instead, use interrupts or DMA DREQ signals.
- Assess whether the default round-robin arbitration performs better than the reachable asymmetric

#### priority configurations.

Fixed by (^) Documentation
DMA

### RP2350-E

#### DMA 1364

Reference (^) RP2350-E
Summary (^) Interactions between CHAIN_TO and ABORT of active channels
Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) The CHAN_ABORT register commands a DMA channel to stop issuing transfers, and to clear its BUSY flag

#### once in-flight transfers have completed. This was originally intended for recovering channels that are

#### stuck with their DREQ low. An ABORT is initiated by writing a bitmap of aborted channels to CHAN_ABORT.

#### Bits remain set until each channel comes to rest.

#### This erratum is a compound of two behaviours: first, aborting a channel will cause its CHAIN_TO to fire, if

#### and only if the aborted channel is the last channel to have completed a write transfer. Second, a channel

#### undergoing an ABORT is susceptible to be re-triggered on the last cycle before the ABORT register clears,

#### because the channel is both inactive and enabled on this cycle, and the ABORT itself doesn’t inhibit

#### triggering. However, since the ABORT is still in effect, the transfer count is held at zero. On the cycle after

#### the ABORT finishes, the channel completes because its transfer counter is zero. This causes the channel’s

#### IRQ and CHAIN_TO to fire on the cycle after the ABORT completes.

#### These two behaviours are problematic when aborting multiple channels that chain to one another, since

#### they may cause the channels to immediately restart post-abort.

Workaround (^) Before aborting an active channel, clear the EN bit (CH0_CTRL_TRIG.EN) of both the aborted channel and

#### any channel it chains to. This ensures the channel isn’t susceptible to re-triggering.

Fixed by (^) Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) CHAIN_TO might not fire for zero-length transfers
Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) The CTRL.CHAIN_TO field configures a channel to start another channel once it completes its programmed

#### sequence of transfers. The CHAIN_TO takes place on the cycle where the channel’s last write completes,

#### and the chainee becomes active on the next cycle.

#### The hardware implementation assumes that CHAIN_TO always happens as a result of a write completion.

#### This isn’t the case when a channel is triggered with a transfer count of zero; in this case the channel

#### completes on the cycle immediately after the trigger without performing any bus accesses.

#### A CHAIN_TO from a channel started with a transfer count of zero will fire if and only if that channel is the

#### last channel to have completed a write transfer. This is true only when the channel in question has

#### previously performed a non-zero-length sequence of transfers, and no other channel has completed a

#### write since.

Workaround (^) Don’t use CHAIN_TO in conjunction with zero-length transfers. Avoid zero-length transfers in the middle of

#### control block lists, and replace them with dummy transfers if possible.

Fixed by (^) Documentation
GPIO

#### GPIO 1365

## Summary  Increased leakage current on Bank 0 GPIO when pad input is enabled

   - RP2350-E
      - Reference RP2350-E
      - Affects RP2350 A
- GPIO

Description (^) For GPIO pads 0 through 47:

#### Increased leakage current when Bank 0 GPIO pads are configured as inputs and the pad is somewhere

#### between VIL and VIH (the undefined logic region).

#### When the pad is set as an input (input enable is enabled and output enable is disabled) and the voltage

#### on the pad is within the undefined logic region, the leakage current exceeds the standard specified IIN

#### leakage level. During this condition the pad can source current (the exact amount is dependent on the

#### chip itself and the exact pad voltage, but typically around 120μA). This leakage will hold the pad at

#### around 2.2 V as that is the effective source voltage of the leakage, and can only be overcome with a

#### suitably low impedance driver / pull.

#### The pad pull-down (if enabled) is significantly weaker than the leakage current in this state and therefore

#### isn’t strong enough to pull the pad voltage low.

#### Driving / pulling the pad input low with a low impedance source of 8.2 kΩ or less will overcome the

#### erroneous leakage and drive the voltage below the level where the leakage current occurs, so in this case

#### if the pad is driven / pulled low it will stay low.

#### The erroneous leakage only occurs (and continues to occur) when the pad input enable is enabled;

#### disabling the input enable will reset (remove) the leakage.

#### The pad pull-up still works. If enabled it will pull the pad to IOVDD as it will pull the input voltage out of the

#### problematic range.

#### The voltages and currents above are based on IOVDD at 3.3 V. For IOVDD at 1.8 V the effective source

#### voltage of the leakage becomes 1.8 V and the peak current is around 30μA. This is effectively a pull-up

#### (separate to the standard pad pull-up) when the pad voltage is between 0.6 V and 1.8 V.

#### These graphs show the leakage current versus pad input voltage for a typical chip for IOVDD at 3.3 V

#### Figure 153 and 1.8 V Figure 154.

#### In detail, this issue presents under the following conditions, for any GPIO 0 through 47:

#### 1. The voltage on the pad is in the undefined logic region.

#### 2. Input buffer is enabled in GPIO0.IE

#### 3. Output buffer is disabled (e.g. selecting the NULL GPIO function)

#### 4. Isolation is clear in GPIO0.ISO, or the previous were true at the point isolation was set

#### When all of the above conditions are met, the input leakage of the pad may exceed the specification.

#### This issue may affect a number of common circuits:

- Relying on floating pins to have a low leakage current
- Relying on the internal pull-down resistor

#### If the internal pull-up is enabled then any floating signal will be pulled high thus removing increased

#### leakage condition as the excess leakage is only sourcing current. This of course can’t prevent the

#### increased leakage if the pad is fed via a strong source e.g. strong potential divider.

#### This doesn’t affect the pull-down behaviour of the pads immediately following a PoR or RUN reset

#### because the input enable field is initially clear. The pull-down resistor functions normally in this state.

#### This issue doesn’t affect the QSPI pads, which use a different pad macro without the faulty circuitry. The

#### USB PHY’s pins are also unaffected.

#### This issue does also affect the SWD pads, which use the same fault-tolerant pad macro as the Bank 0

#### GPIOs. However, both SWD pads are pull-up by default, so there is no ill effect.

#### GPIO 1367

Workaround (^) If pad pull-down behaviour is required, clear the pad input enable in GPIO0.IE (for GPIOs 0 through 47) to

#### ensure that the pad pull-down resistor pulls the pad signal low. To read the state of a pad pulled-down

#### GPIO from software, enable the input buffer by setting GPIO0.IE immediately before reading, and then re-

#### disable immediately afterwards. If the pad is already a logic-0, re-enabling the input doesn’t disturb the

#### pull-down state.

#### Alternatively an external pull-down of 8.2 kΩ or less can be used.

#### PIO programs can’t toggle pad controls and therefore external pulls may be required, depending on your

#### application.

#### As normal, if ADC channels are being used on a pin, clear the relevant GPIO input enable as stated in

#### Section 12.4.3.

Fixed by (^) RP2350 A3, Documentation
Figure 153. GPIO Pad
leakage for
IOVDD=3.3 V
Figure 154. GPIO Pad
leakage for
IOVDD=1.8 V
Hazard

### RP2350-E

#### Reference RP2350-E

#### Hazard3 1368

Summary (^) System Bus Access stalls indefinitely when core 1 is in clock-gated sleep
Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) System Bus Access (SBA) is a RISC-V debug feature that allows the Debug Module direct access to the

#### system bus, independent of the state of harts in the system. RP2350 implements SBA by arbitrating

#### Debug Module bus accesses with the core 1 load/store port.

#### Hazard3 implements custom low-power states controlled by the MSLEEP CSR. When

#### MSLEEP.DEEPSLEEP is set, Hazard3 completely gates its clock, with the exception of the minimal logic

#### required to wake again. Due to a design oversight, this also clock-gates the arbiter between SBA and

#### load/store bus access. (This is addressed in upstream commit c11581e.)

#### Consequently, if you initiate an SBA transfer whilst MSLEEP.DEEPSLEEP is set on core 1, and core 1 is in

#### a WFI-equivalent sleep state, the SBA transfer will make no progress until core 1 wakes from the WFI

#### state. The processor wakes upon an enabled interrupt being asserted, or a debug halt request.

Workaround (^) Either configure your debug translator to not use SBA, or don’t enter clock-gated sleep on core 1. The A

#### bootrom mitigates this issue by not setting DEEPSLEEP in the initial core 1 wait-for-launch code.

#### The processors are synthesised with hierarchical clock gating, so the top-level clock gate controlled by

#### the DEEPSLEEP flag brings minimal power savings over a default WFI sleep state.

Fixed by (^) Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) PMPCFGx RWX fields are transposed
Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) The Physical Memory Protection unit (PMP) defines read, write and execute permissions (RWX) for

#### configurable ranges of physical memory. The RWX permissions for four regions are packed into each 32-

#### bit PMPCFG register, PMPCFG0 through PMPCFG3.

#### Per the RISC-V privileged ISA specification, the permission fields are ordered X, W, R from MSB to LSB.

#### Hazard3 implements them in the order R, W, X. This means software using the correct bit order will have

#### its read permissions applied as execute, and vice versa. (See upstream commit 7d37029.)

Workaround (^) When configuring PMP with X != R, use the bit order implemented by this version of Hazard3. In the SDK,

#### the hardware/regs/rvcsr.h register header provides bitfield definitions for the as-implemented order when

#### building for RP2350.

Fixed by (^) Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) U-mode doesn’t ignore mstatus.mie
Affects (^) RP2350 A2, RP2350 A3, RP2350 A

#### Hazard3 1369

Description (^) The MSTATUS.MIE bit is a global enable for interrupts that target M-mode. Software generally clears this

#### momentarily to ensure short critical sections are atomic with respect to interrupt handlers.

#### The RISC-V privileged ISA specification requires that the interrupt enable flag for a given privilege mode is

#### treated as 1 when the hart is in a lower privilege mode. In this case, mstatus.mie should be treated as 1

#### when the core is in U-mode.

#### Hazard3 doesn’t implement this rule, so entering U-mode with M-mode interrupts disabled results in no

#### M-mode interrupts being taken. (See upstream commit a84742a.)

Workaround (^) When returning to U-mode from M-mode via an mret with mstatus.mpp == 0 , ensure mstatus.mpie is set, so

#### that IRQs will be enabled by the return.

Fixed by (^) Documentation
OTP

### RP2350-E

Reference (^) RP2350-E
Summary (^) USB_OTP_VDD disruption can result in corrupt OTP row read data
Affects (^) RP2350 A
Description (^) The OTP array has a read voltage generated from USB_OTP_VDD using an internal linear regulator. While the

#### regulator has a "power good" signal, it isn’t sampled outside of the initial power-on reset startup

#### sequence. External manipulation of USB_OTP_VDD can result in incorrect data being latched during the array

#### read phase.

#### The erroneous behaviour includes, but isn’t limited to:

- Latching the previous read cycle data from the array
- One or many bitlines returning zeroes for programmed bits
- Byte-shifted read data

#### In the case of guarded reads, the first failure mode can result in the guard read check passing and the

#### guard word also ending up as the read data. If the critical data are the CRIT0/CRIT1 flags, sampled by the

#### OTP PSM during boot, this can enable Hazard3 debug and disable the Arm cores, which results in a

#### reversion of the effects of the CRIT1.SECURE_BOOT_ENABLE and CRIT1.DEBUG_DISABLE flags.

#### Guarded ECC reads aren’t typically vulnerable to corruption of this nature as the guard word is an invalid

#### ECC word, and bit deletion or byte shifting reliably invalidates the ECC check.

#### RP2350 A3 incorporates more safeguards against erroneous OTP behaviour. If any of the following

#### checks fail, the chip is reset back to the start of the OTP PSM stage.

- The OTP regulator OK signal is continuously checked whenever OTP PSM or user accesses are

#### being performed.

- Bit 0 of the row read address selects either the first or second ROM calibration word (0x333333^ or

#### 0xcccccc) for any guarded read, and is validated accordingly.

- Reserved-0 bits in the CRIT0/1 rows are checked as reading 0 in the OTP PSM.

#### These checks are performed regardless of the security state of the chip. The OTP regulator check may be

#### masked with bit 2 of the AUXCTRL register.

Credit (^) Aedan Cullen (see https://github.com/aedancullen/hacking-the-rp2350)

#### OTP 1370

Workaround (^) None
Fixed by (^) RP2350 A

### RP2350-E

Reference (^) RP2350-E
Summary (^) Performing a guarded read on a single ECC OTP row causes a fault if the data in the adjacent row isn’t

#### also valid ECC data.

Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) Each "ECC row" in OTP stores 16 bits of user data along with error correction information used to correct

#### and/or detect bit errors. ECC rows are used to store data value, which are written a full 16 bits at a time

#### into OTP.

#### A "guarded" ECC row read is intended to be used by the RP2350 boot path, or other Secure software when

#### it expects to read an ECC row and can’t proceed if the row value is invalid. Reading such an invalid ECC

#### value through a "guarded" read halts the chip until it’s rebooted.

#### If ECC row programming is interrupted, an ECC row might contain a value that fails ECC validation.

#### Because any ECC could potentially contain an invalid, partially written value, the bootrom uses a separate

#### "enable" flag in OTP to indicate whether a particular ECC row is expected to contain a valid value. The

#### user is expected to only set this flag after a particular ECC row is known to have been written correctly.

#### The RP2350 OTP hardware actually reads a pair of rows (starting on the even row) whenever an ECC read

#### is performed but only returns one row value. When performing a guarded ECC read, it actually checks

#### both rows validity, so the guarded read can cause a halt if either row in the pair isn’t a valid ECC value.

Workaround (^) • Never store ECC rows and RAW rows in the same pair of rows (a pair of rows starting on an even

#### row number), since the RAW row is unlikely to always contain a valid ECC value. Note however that

#### zero in a RAW row is a valid ECC value.

- Never store two ECC rows in the same pair of rows if they are protected by different "enable" flags.

#### This workaround is fine for user use of OTP, however certain pre-existing ECC row pairs used by the

#### bootrom violate workaround 2:

- FLASH_DEVINFO and FLASH_PARTITION_SLOT_SIZE
- BOOTSEL_LED_CFG and BOOTSEL_PLL_CFG

#### To be absolutely safe, don’t update and set the "enable" flag for one half of the pair after you have set the

#### "enable" flag for the other half. If you want to set both ECC values safely, set them both, then set both

#### "enable" flags.

Fixed by (^) Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) OTP keys for pages 62/63 are applied to all lock words 0 through 63
Affects (^) RP2350 A2, RP2350 A3, RP2350 A

#### OTP 1371

Description (^) As described in Section 13.5, the uppermost 64 words (128 rows) of OTP contain protection information

#### for each 128-byte page of OTP. The total ECC data capacity of the OTP is 64 × 128 B = 8192 B, so there is

#### one such lock word for each page. The permissions in each lock word n cover OTP rows 64 * n through 64

#### * n + 63 (inclusive), and they also cover the lock word itself.

#### This makes lock words 62 and 63 special because they don’t have any associated OTP page. This is

#### because those pages would overlap with the locations where the lock words are stored. Instead, lock

#### words 62 and 63 should only protect themselves. This rule is applied correctly for the effects of LOCK_NS

#### and LOCK_S bits. However, the protection checks for the KEY_R, KEY_W, and NO_KEY_STATE bits don’t handle

#### pages 62 and 63 correctly. Instead, they simply divide the row number by 64 to look up the lock word.

#### The effect is that lock words 0 through 31 have a key protection state defined by PAGE62_LOCK0, and

#### lock words 32 through 63 have a key protection state defined by PAGE63_LOCK0.

#### Conversely, the key configuration in lock words 0 through 61 does not affect the accessibility of those

#### lock words. It only affects the accessibility of the actual data pages protected by those lock words.

Workaround (^) As a partial mitigation, factory programming revokes Non-secure write permission to pages 62 and 63 on

#### all devices. This avoids Non-secure software disabling Secure access to lock words by deliberately

#### installing an invalid key. For the full list of permissions pre-programmed on blank devices, see Section

#### 13.5.5. This mitigation is applied on all versions of RP2350.

#### Software shouldn’t rely on OTP access keys for protection of lock words.

Fixed by (^) Documentation
RCP

### RP2350-E

Reference (^) RP2350-E
Summary (^) RCP random delays can create a side-channel
Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) The RCP delay is implemented as a coprocessor stall; this has the effect of completely pausing the

#### associated core. As the core is effectively halted for the duration of the delay, this represents a

#### significant reduction in gate toggle activity across the chip if there are no other bus managers active (e.g.

#### other CPU or DMA). The reduction in toggle activity causes a reduction in DVDD current, and the typical

#### length of the delay means that the reduction is measureable outside of the chip. The reduction in current

#### and subsequent increase may create a fault injection trigger point. Instructions immediately after an RCP

#### delay operation can be more reliably targeted, undoing the cumulative effect of clock randomisation.

#### A second-order effect of the RCP delay probability distribution is that after N RCP instructions for large N,

#### the added latency converges to a normal distribution centred on N * 63 cycles. Therefore, instructions

#### after a known number of RCP delays are statistically easier to target.

#### With these two factors in mind, programmers should use RCP delays in Secure code with great care. In

#### particular, avoid using RCP delays:

- Inside inner loops that may be executed many times.
- As part of boilerplate assembly in function prologues/epilogues.
- Immediately prior to particularly critical actions, such as modifying^ ACCESSCTRL.

#### As a mitigation, as of RP2350 A3, the bootrom uses the non-delay variant for all RCP instructions.

Workaround (^) Use of the non-delay RCP instruction variant is recommended.

#### RCP 1372

Fixed by (^) Documentation, Software
SIO

### RP2350-E

Reference (^) RP2350-E
Summary (^) Interpolator OVERF bits are broken by new right-rotate behaviour
Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) RP2350 replaces the interpolator right-shift with a right-rotate, so that left shifts can be synthesised. This

#### is useful for scaled indexed addressing in tight address-generating loops.

#### The OVERF flag functions by checking for nonzero bits in the post-shift value that have been masked out by

#### the MSB mask configured by the CTRL_LANE0_MASK_MSB and CTRL_LANE1_MASK_MSB register fields. This is used to

#### discard samples outside of the [0, 1) wrapping domain of UV coordinates represented by ACCUM0 and

#### ACCUM1, for example in affine-transformed sprite sampling.

#### The issue occurs because the right-rotate causes nonzero LSBs to be rotated up to the MSBs. These

#### nonzero bits spuriously set the OVERF flag.

Workaround (^) Either compute OVERF manually by checking the ACCUM0/ACCUM1 MSBs, or precompute the bounds in advance

#### to avoid per-sample checks.

Fixed by (^) Documentation

### RP2350-E

Reference (^) RP2350-E
Summary (^) SIO SPINLOCK writes are mirrored at +0x80 offset
Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) The SIO contains spinlock registers, SPINLOCK0 through SPINLOCK31. Reading a spinlock register

#### attempts to claim it, returning nonzero if the claim was successful and 0 if unsuccessful. Writing to a

#### spinlock register releases it, so the next claim will be successful. SIO spinlock registers are at register

#### offsets 0x100 through 0x17c within SIO.

#### RP2350 adds new SIO registers at register offsets 0x180 and above: Doorbells, the PERI_NONSEC register,

#### the RISC-V soft IRQ register, the RISC-V MTIME registers, and the TMDS encoder.

#### The SIO address decoder detects writes to spinlocks by decoding on bit 8 of the address. This means

#### writes in the range 0x180 through 0x1fc are spuriously detected as writes to the corresponding spinlock

#### address 128 bytes below, in the range 0x100 through 0x17c. Writing to any of these high registers will set

#### the corresponding lock to the unclaimed state.

#### This erratum only affects writes to the spinlock registers. Reads are correctly decoded, so aren’t affected

#### by accesses above 0x17c.

#### SIO 1373

Workaround (^) Use processor atomic instructions instead of the SIO spinlocks. The SDK hardware_sync_spin_lock library

#### uses software lock variables by default when building for RP2350, instead of hardware spinlocks.

#### The following SIO spinlocks can be used normally because they don’t alias with writable registers: 5, 6, 7,

#### 10, 11, and 18 through 31. Some of the other lock addresses may be used safely depending on which of

#### the high-addressed SIO registers are in use.

#### Locks 18 through 24 alias with some read-only TMDS encoder registers, which is safe as only writes are

#### mis-decoded.

Fixed by (^) Documentation, Software
XIP

### RP2350-E

Reference (^) RP2350-E
Summary (^) XIP cache clean by set/way operation modifies the tag of dirty lines
Affects (^) RP2350 A2, RP2350 A3, RP2350 A
Description (^) The 0x1 clean by set/way cache maintenance operation performs the following steps:

#### 1. Selects a cache line: address bits 12:3 index the cache sets, and bit 13 selects from the two 8-byte

#### cache lines, which make up the ways of each set.

#### 2. Checks if the line contains uncommitted write data (a dirty line).

#### 3. If the line is dirty, writes the data downstream and marks the line as clean.

#### In the third step, in addition to marking the line as clean, the cache controller erroneously sets the cache

#### line’s tag to address bits 25:13 of the maintenance write that initiated the clean operation. The cache uses

#### the tag to recall which of the many possible downstream addresses currently resides in each cache line.

#### Therefore reading the newly tagged address returns cached data from the original address, breaking the

#### memory contract.

#### Consider the following example scenario:

- QMI window 0 (starting at^ 0x10000000) has a flash device attached
- QMI window 1 (starting at^ 0x11000000) has a PSRAM device attached
- The cache possesses address^ 0x11000000^ in the dirty state, and it is allocated in way 0 of the cache

#### The programmer cleans the cache, starting by writing to address 0x18000001 to clean set 0, way 0. This

#### cleans the dirty line containing address 0x11000000. After cleaning, the cache updates this line’s tag to all-

#### zeroes (the offset of the maintenance write). A subsequent read from 0x10000000 results in a spurious

#### cache hit, returning PSRAM data in place of flash data.

#### See Section 4.4.1.1 for more information about cache maintenance operations. See Section 4.4.1.2 for

#### more information about cache line states and state transitions.

#### The tag update only affects 0x1 clean by set/way; is either correct or harmless for the other four cache

#### maintenance operations.

#### XIP 1374

Workaround (^) To avoid spurious cache hits, choose an address that can’t alias with cached data from the QMI. This

#### remaps dirty lines outside of the QMI address space after cleaning them, which has the side effect of

#### causing a cache miss on the next access to the dirty address. The SDK xip_cache_clean_all() function

#### implements this workaround.

#### The updated tag is predictable: it is always the address of the maintenance write. For example, use the

#### upper 16 kB of the maintenance space to clean all cache lines:

```
1 volatile uint8_t *maintenance_ptr = (volatile uint8_t*)0x1bffc001u;
2 for (int i = 0; i < 0x4000; i += 8) {
3 maintenance_ptr[i] = 0;
4 }
```
#### Because the clean operation is a no-op for invalid, clean or pinned lines, this workaround doesn’t interfere

#### with lines pinned for cache-as-SRAM use.

Fixed by (^) Documentation, Software
USB

### RP2350-E

Reference (^) RP2350-E
Summary (^) Inadequate synchronisation of USB status signals
Affects (^) RP2350 A2, RP2350 A3, RP2350 A4 (mitigated on A3)

#### USB 1375

Description (^) Within the USB peripheral, certain Host and Device controller events cross from clk_usb to clk_sys. Many

#### of these signals don’t have appropriate synchronisation methods to ensure that they are correctly

#### registered when clk_sys is equal to or slower than clk_usb.

#### The following signals lack appropriate synchronisation methods:

#### SIE_STATUS:

- TRANS_COMPLETE
- SETUP_REC
- STALL_REC
- NAK_REC
- RX_SHORT_PACKET
- ACK_REQ
- DATA_SEQ_ERROR
- RX_OVERFLOW

#### INTR:

- HOST_SOF
- ERROR_CRC
- ERROR_BIT_STUFF
- ERROR_RX_OVERFLOW
- ERROR_RX_TIMEOUT
- ERROR_DATA_SEQ

#### The bootrom’s USB bootloader derives clk_sys from pll_usb. Therefore, the two clock frequencies are

#### identical and have a fixed phase relationship. Under this condition, and at extremes of PVT, lab testing

#### has shown that these events can be lost, which results in unreliable USB bootloader behaviour.

#### RP2350 A3 incorporates hardware fixes that improve timing margins on signals critical to the bootrom,

#### ensuring reliable operation across PVT. However, software must not rely on these fixes, and so they

#### aren’t elaborated on.

Workaround (^) Run clk_sys faster than clk_usb by at least 10% when the peripheral is in use. Signalling of quasi-static bus

#### states such as reset, suspend, and resume aren’t affected by this erratum, so clk_sys can be lower in

#### these cases.

Fixed by (^) Documentation, Software

#### USB 1376

