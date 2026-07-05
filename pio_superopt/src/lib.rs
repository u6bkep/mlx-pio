//! Simulated-annealing superoptimizer for RP2350 PIO programs.
//!
//! This crate currently provides the **symbolic IR**: a total,
//! legal-by-construction representation of PIO instructions and the
//! per-state-machine config that affects encoding, plus an [`encode`]
//! pass to the 16-bit machine format.
//!
//! Design (see project notes):
//!   * The search subject is a fixed 32-slot program; jump/wrap targets
//!     are slot indices, so addresses never shift under mutation.
//!   * `delay` and `sideset` are stored **config-independent** per
//!     instruction; the shared 5-bit delay/side-set budget is resolved
//!     only at encode time against the SM's [`ir::SideCfg`].
//!   * The operand enums enumerate exactly the *legal* field values, so
//!     every value in the IR encodes to a legal instruction and reserved
//!     bit patterns are unrepresentable.

pub mod certify;
pub mod cost;
pub mod decode;
pub mod encode;
pub mod fixtures;
pub mod gene;
pub mod gene_search;
pub mod ir;
pub mod program;
pub mod rng;
pub mod run;
pub mod search;
pub mod spec_cost;

pub use cost::{hamming, hamming_masked, hamming_tolerant, score, score_masked, Score};
pub use decode::{decode_insn, DecodeError};
pub use encode::encode_insn;
pub use ir::{Insn, Op, SideCfg};
pub use program::{import_program, Config, PinMap, Program, ShiftCfg, ShiftDir};
pub use run::{configure, run, run_full, RunSpec};
