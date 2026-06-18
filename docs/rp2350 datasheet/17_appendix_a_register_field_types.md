# Appendix A: Register field types

## Changes from RP

## Register field types are unchanged.

## Standard types

## RW:

## • Read/Write

## • Read operation returns the register value

## • Write operation updates the register value

## RO:

## • Read-only

## • Read operation returns the register value

## • Write operations are ignored

## WO:

## • Write-only

## • Read operation returns 0

## • Write operation updates the register value

## Clear types

## SC:

## • Self-Clearing

## • Writing a 1 to a bit in an SC field will trigger an event, once the event is triggered the bit clears automatically

## • Writing a 0 to a bit in an SC field does nothing

## WC:

## • Write-Clear

## • Writing a 1 to a bit in a WC field will write that bit to 0

## RP2350 Datasheet

## Changes from RP2040 1349

- Writing a 0 to a bit in a WC field does nothing
- Read operation returns the register value

## FIFO types

##### These fields are used for reading and writing data to and from FIFOs. Accompanying registers provide FIFO control and

##### status. There is no fixed format for the control and status registers, as they are specific to each FIFO interface.

#### RWF:

- Read/Write FIFO
- Reading this field returns data from a FIFO

### ◦ When the read is complete, the data value is removed from the FIFO

### ◦ If the FIFO is empty, a default value will be returned; the default value is specific to each FIFO interface

- Data written to this field is pushed to a FIFO, Behaviour when the FIFO is full is specific to each FIFO interface
- Read and write operations may access different FIFOs

#### RF:

- Read FIFO
- Functions the same as RWF, but read-only

#### WF:

- Write FIFO
- Functions the same as RWF, but write-only

##### RP2350 Datasheet

##### FIFO types 1350

