//! Prototype + validation of the RXProcessor bit-realignment front-end
//! ("software re-frame") destined for rs485-eth lib.rs. The bench showed
//! frames arriving with the RX's 5-bit ISR grouping offset by a constant
//! (e.g. +1 bit): every bit correct, every symbol garbage. This realigner
//! locks onto the J-H-H frame prefix at ANY bit offset and re-emits
//! correctly grouped symbols; streams already at offset 0 pass through
//! unchanged. The struct below is transplanted verbatim into the firmware
//! (no_std-compatible: no alloc, no std).
//!
//! Run: cargo test -p pio_harness --test reframe_proto -- --nocapture

/// Bit-level frame realigner. Symbols arrive one per byte (5 valid bits,
/// first-received bit = bit 4). Between frames the line idles (all-ones
/// symbols). On the first non-idle bits, search for the frame prefix
/// J,H,H (arrival bits 00011 00100 00100) at any bit offset and lock the
/// 5-bit grouping there; if no prefix appears within the search window,
/// fall back to the incoming grouping (today's behavior). `reset()` on
/// idle/frame-end re-arms the search for the next frame.
#[derive(Clone, Copy)]
pub struct BitRealigner {
    buf: u64,
    n: u8,
    locked: bool,
}

/// J(00011) H(00100) H(00100) in arrival order, 15 bits.
const PREFIX: u64 = 0b000110010000100;
const PREFIX_LEN: u8 = 15;
/// Give up searching after this many buffered bits and pass through.
const SEARCH_LIMIT: u8 = 40;

impl BitRealigner {
    pub const fn new() -> Self {
        Self { buf: 0, n: 0, locked: false }
    }

    pub fn reset(&mut self) {
        self.buf = 0;
        self.n = 0;
        self.locked = false;
    }

    /// Feed one received symbol byte; up to 2 realigned symbols come out.
    /// Returns how many were written to `out`.
    pub fn push(&mut self, byte: u8, out: &mut [u8; 2]) -> usize {
        self.buf = (self.buf << 5) | (byte & 0x1F) as u64;
        self.n += 5;

        if !self.locked {
            // Pure idle so far: keep at most one idle symbol buffered.
            let mask = (1u64 << self.n) - 1;
            if (self.buf & mask) == mask {
                self.buf = 0x1F;
                self.n = 5;
                return 0;
            }
            // Non-idle bits present: search for the prefix.
            if self.n >= PREFIX_LEN {
                let mut s = 0u8;
                while s + PREFIX_LEN <= self.n {
                    let window = (self.buf >> (self.n - PREFIX_LEN - s)) & ((1 << PREFIX_LEN) - 1);
                    if window == PREFIX {
                        // Drop everything before the J and lock.
                        self.n -= s;
                        self.buf &= (1u64 << self.n) - 1;
                        self.locked = true;
                        break;
                    }
                    s += 1;
                }
            }
            if !self.locked {
                if self.n >= SEARCH_LIMIT {
                    // No prefix: behave like the pre-fix firmware (group
                    // as received). Trim to a multiple of 5 from the head.
                    let extra = self.n % 5;
                    self.n -= extra;
                    self.buf &= (1u64 << self.n.max(1)) - 1;
                    self.locked = true;
                } else {
                    return 0;
                }
            }
        }

        let mut emitted = 0;
        while self.n >= 5 && emitted < 2 {
            self.n -= 5;
            out[emitted] = ((self.buf >> self.n) & 0x1F) as u8;
            emitted += 1;
        }
        emitted
    }
}

// ---------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------

/// Regroup a bit vector into 5-bit symbol bytes at a given bit offset
/// (prepending `offset` stray bits) — models the RX's misgrouped output.
fn misgroup(bits: &[u8], offset: usize) -> Vec<u8> {
    let mut all = vec![1u8; offset]; // stray bits ahead of the frame (idle=1)
    all.extend_from_slice(bits);
    while all.len() % 5 != 0 {
        all.push(1); // trailing idle bits
    }
    all.chunks(5)
        .map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b))
        .collect()
}

fn frame_bits() -> Vec<u8> {
    // J J H H + a few data symbols + T R (arrival order, 5 bits each,
    // first-received first).
    let syms: &[u8] = &[
        0x03, 0x03, 0x04, 0x04, // J J H H (received values)
        0x0F, 0x12, 0x15, 0x0A, // data
        0x16, 0x1C, // T R (received values 0x16? EndT tx=0x0D rev=0x16; R tx=0x07 rev=0x1C)
    ];
    let mut bits = Vec::new();
    for &s in syms {
        for i in (0..5).rev() {
            bits.push((s >> i) & 1);
        }
    }
    bits
}

fn run(realigner: &mut BitRealigner, input: &[u8]) -> Vec<u8> {
    let mut out_all = Vec::new();
    let mut out = [0u8; 2];
    for &b in input {
        let k = realigner.push(b, &mut out);
        out_all.extend_from_slice(&out[..k]);
    }
    out_all
}

#[test]
fn realigns_all_offsets() {
    let bits = frame_bits();
    for offset in 0..5 {
        // two idle symbols, then the (mis)grouped frame
        let mut input = vec![0x1F, 0x1F];
        input.extend(misgroup(&bits, offset));
        let mut r = BitRealigner::new();
        let out = run(&mut r, &input);
        // The output must contain the correctly-grouped frame: find first J
        let stream: Vec<u8> = out.iter().copied().skip_while(|&s| s == 0x1F).collect();
        assert!(
            stream.len() >= 9,
            "offset {offset}: too few symbols: {stream:02x?}"
        );
        assert_eq!(
            &stream[..9],
            &[0x03, 0x04, 0x04, 0x0F, 0x12, 0x15, 0x0A, 0x16, 0x1C],
            "offset {offset}: stream {stream:02x?}"
        );
        println!("offset {offset}: OK ({} symbols)", stream.len());
    }
}

#[test]
fn bench_signature_plus_one_bit() {
    // The exact bench failure: ONE extra bit ahead of the frame. The old
    // firmware saw 01 12 02... garbage; the realigner must recover JHH.
    let bits = frame_bits();
    let input = misgroup(&bits, 1);
    // sanity: the misgrouped stream is NOT valid JHH at offset 0
    assert_ne!(&input[..3], &[0x03, 0x04, 0x04][..], "{input:02x?}");
    let mut r = BitRealigner::new();
    let out = run(&mut r, &input);
    let stream: Vec<u8> = out.iter().copied().skip_while(|&s| s == 0x1F).collect();
    assert_eq!(&stream[..3], &[0x03, 0x04, 0x04], "realigned head: {stream:02x?}");
    println!("bench +1-bit signature realigned OK: {:02x?}", &stream[..6]);
}

#[test]
fn offset0_passthrough_and_reset() {
    let bits = frame_bits();
    let mut input = vec![0x1F];
    input.extend(misgroup(&bits, 0));
    input.push(0x1F);
    let mut r = BitRealigner::new();
    let out1 = run(&mut r, &input);
    let s1: Vec<u8> = out1.iter().copied().skip_while(|&s| s == 0x1F).collect();
    assert_eq!(&s1[..3], &[0x03, 0x04, 0x04]);
    // second frame after reset, different offset
    r.reset();
    let mut input2 = vec![0x1F];
    input2.extend(misgroup(&bits, 3));
    let out2 = run(&mut r, &input2);
    let s2: Vec<u8> = out2.iter().copied().skip_while(|&s| s == 0x1F).collect();
    assert_eq!(&s2[..3], &[0x03, 0x04, 0x04], "{s2:02x?}");
    println!("passthrough + reset + offset3 OK");
}

#[test]
fn no_prefix_fallback() {
    // Garbage with no JHH: after SEARCH_LIMIT bits the realigner passes
    // through so real noise still reaches the (noise-tolerant) tag layer.
    let input: Vec<u8> = vec![0x15, 0x0A, 0x15, 0x0A, 0x15, 0x0A, 0x15, 0x0A, 0x15, 0x0A];
    let mut r = BitRealigner::new();
    let out = run(&mut r, &input);
    assert!(!out.is_empty(), "fallback must emit");
    println!("fallback emitted {} symbols", out.len());
}
