//! Shard-coordination for fleet enumeration (`superopt serve` / `superopt
//! work`): a tiny lease server so hosts can pull shards dynamically instead
//! of the static `--shard-mod/--shard-rem` split (which remains the
//! no-server fallback — see `docs/fleet.md`).
//!
//! Split in two layers:
//!
//!   * [`Coordinator`] — pure lease bookkeeping over the shard universe
//!     `0..total`. No sockets, no filesystem; time comes in as an explicit
//!     [`Instant`] so expiry is unit-testable.
//!   * [`serve`] — the thin HTTP glue (tiny_http) the `serve` subcommand and
//!     the e2e test both drive. All durable state is the `shard-NNNN.json`
//!     files in the out dir (written atomically, tmp+rename), exactly like
//!     the single-machine driver — so killing the server loses nothing:
//!     restart it, it rescans the dir, workers re-lease what's left.
//!
//! Protocol (JSON bodies, serde_json):
//!
//!   * `GET  /status` -> `{"len":N,"alphabet":A,"done":d,"leased":l,"remaining":r}`
//!   * `POST /lease` body `{"len":N,"alphabet":A}` -> 200 `{"shard":s}`,
//!     204 when nothing is leasable, 409 when len/alphabet mismatch the
//!     server's (the shard-numbering contract — mixed binaries are refused).
//!   * `POST /done?shard=S` body = the `ShardResult` JSON verbatim -> 200.
//!     The body must parse as JSON with a `shard` field equal to S. Late
//!     results (lease expired, shard re-leased elsewhere) are accepted as
//!     long as the shard is not already done; a duplicate for a done shard
//!     is discarded with 200 (idempotent).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Lease bookkeeping over shards `0..total`. See module doc.
pub struct Coordinator {
    total: usize,
    done: HashSet<usize>,
    /// shard -> when it was leased. Entries are removed on completion;
    /// expired entries may linger until re-leased (harmless — [`lease`] and
    /// [`counts`] both treat them as free).
    leases: HashMap<usize, Instant>,
    lease_ttl: Duration,
}

impl Coordinator {
    pub fn new(total: usize, lease_ttl: Duration) -> Self {
        Coordinator { total, done: HashSet::new(), leases: HashMap::new(), lease_ttl }
    }

    /// Mark a shard done without a lease round-trip: used for the startup
    /// rescan of existing `shard-*.json` files. Clears any lease.
    pub fn mark_done(&mut self, shard: usize) {
        self.leases.remove(&shard);
        self.done.insert(shard);
    }

    /// Lowest shard that is not done and not under an unexpired lease;
    /// records the lease at `now`. `None` when everything is done or
    /// actively leased.
    pub fn lease(&mut self, now: Instant) -> Option<usize> {
        for shard in 0..self.total {
            if self.done.contains(&shard) {
                continue;
            }
            if let Some(&t) = self.leases.get(&shard) {
                if now.duration_since(t) < self.lease_ttl {
                    continue; // active lease
                }
            }
            self.leases.insert(shard, now);
            return Some(shard);
        }
        None
    }

    /// Record a completion. Returns whether the shard was still outstanding
    /// (false = it was already done; the caller should discard the duplicate
    /// result). Clears any lease either way.
    pub fn complete(&mut self, shard: usize) -> bool {
        self.leases.remove(&shard);
        self.done.insert(shard)
    }

    /// `(done, leased_active, remaining)` at `now`. `leased_active` counts
    /// only unexpired leases on not-done shards; `remaining` is what neither
    /// of the other buckets holds (free to lease right now).
    pub fn counts(&self, now: Instant) -> (usize, usize, usize) {
        let done = self.done.len();
        let leased = self
            .leases
            .iter()
            .filter(|(s, &t)| !self.done.contains(s) && now.duration_since(t) < self.lease_ttl)
            .count();
        (done, leased, self.total - done - leased)
    }
}

/// Static config for the HTTP loop.
pub struct ServeCfg {
    pub len: usize,
    pub alphabet: usize,
    pub out_dir: PathBuf,
}

/// Run the coordinator HTTP loop forever on an already-bound server. The
/// `serve` subcommand and the e2e test both call this; port choice and the
/// startup rescan are the caller's job. Single-threaded on purpose —
/// requests are tiny and rare (one lease + one done per shard, and a shard
/// is minutes-to-hours of work).
pub fn serve(server: &tiny_http::Server, coord: &mut Coordinator, cfg: &ServeCfg) -> ! {
    let t0 = Instant::now();
    let mut last_progress = Instant::now();
    loop {
        // 1s tick so the periodic progress line fires even when idle.
        let req = match server.recv_timeout(Duration::from_secs(1)) {
            Ok(Some(r)) => Some(r),
            Ok(None) => None,
            Err(e) => {
                eprintln!("coord: accept error: {e}");
                None
            }
        };
        if let Some(req) = req {
            handle(req, coord, cfg, t0);
        }
        let now = Instant::now();
        let (done, leased, remaining) = coord.counts(now);
        if leased > 0 && now.duration_since(last_progress) >= Duration::from_secs(60) {
            last_progress = now;
            eprintln!(
                "coord: {done} done, {leased} leased, {remaining} remaining ({:.0}s elapsed)",
                t0.elapsed().as_secs_f64()
            );
        }
    }
}

fn respond_json(req: tiny_http::Request, code: u16, body: String) {
    let resp = tiny_http::Response::from_string(body).with_status_code(code);
    if let Err(e) = req.respond(resp) {
        eprintln!("coord: respond error: {e}");
    }
}

fn handle(mut req: tiny_http::Request, coord: &mut Coordinator, cfg: &ServeCfg, t0: Instant) {
    use tiny_http::Method;
    let url = req.url().to_string();
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (url.clone(), None),
    };
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        respond_json(req, 400, "{\"error\":\"unreadable body\"}".into());
        return;
    }
    let now = Instant::now();
    match (req.method().clone(), path.as_str()) {
        (Method::Get, "/status") => {
            let (done, leased, remaining) = coord.counts(now);
            respond_json(
                req,
                200,
                format!(
                    "{{\"len\":{},\"alphabet\":{},\"done\":{done},\"leased\":{leased},\"remaining\":{remaining}}}",
                    cfg.len, cfg.alphabet
                ),
            );
        }
        (Method::Post, "/lease") => {
            let v: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    respond_json(req, 400, format!("{{\"error\":\"bad lease body: {e}\"}}"));
                    return;
                }
            };
            let (wlen, walpha) = (v["len"].as_u64(), v["alphabet"].as_u64());
            if wlen != Some(cfg.len as u64) || walpha != Some(cfg.alphabet as u64) {
                // The shard-numbering contract: a worker built from another
                // rev (different alphabet order/size) must be refused.
                respond_json(
                    req,
                    409,
                    format!(
                        "{{\"error\":\"contract mismatch: server len={} alphabet={}, worker len={:?} alphabet={:?} — same commit/binary everywhere\"}}",
                        cfg.len, cfg.alphabet, wlen, walpha
                    ),
                );
                return;
            }
            match coord.lease(now) {
                Some(shard) => {
                    let (done, leased, remaining) = coord.counts(now);
                    eprintln!(
                        "[shard {shard:04}] leased  ({done} done, {leased} leased, {remaining} remaining, {:.0}s elapsed)",
                        t0.elapsed().as_secs_f64()
                    );
                    respond_json(req, 200, format!("{{\"shard\":{shard}}}"));
                }
                None => respond_json(req, 204, String::new()),
            }
        }
        (Method::Post, "/done") => {
            let shard: usize = match query
                .as_deref()
                .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("shard=")))
                .and_then(|s| s.parse().ok())
            {
                Some(s) => s,
                None => {
                    respond_json(req, 400, "{\"error\":\"missing/bad ?shard=\"}".into());
                    return;
                }
            };
            let v: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    respond_json(req, 400, format!("{{\"error\":\"result is not JSON: {e}\"}}"));
                    return;
                }
            };
            if v["shard"].as_u64() != Some(shard as u64) {
                respond_json(
                    req,
                    400,
                    format!("{{\"error\":\"body shard {:?} != query shard {shard}\"}}", v["shard"]),
                );
                return;
            }
            if !coord.complete(shard) {
                // Already done (duplicate from a late host) — idempotent.
                respond_json(req, 200, "{\"status\":\"duplicate, discarded\"}".into());
                return;
            }
            let path = cfg.out_dir.join(format!("shard-{shard:04}.json"));
            let tmp = cfg.out_dir.join(format!("shard-{shard:04}.json.tmp"));
            if let Err(e) = std::fs::write(&tmp, &body).and_then(|_| std::fs::rename(&tmp, &path)) {
                eprintln!("coord: FAILED to write {}: {e}", path.display());
                respond_json(req, 500, format!("{{\"error\":\"write failed: {e}\"}}"));
                return;
            }
            let (done, leased, remaining) = coord.counts(now);
            eprintln!(
                "[shard {shard:04}] done    ({done} done, {leased} leased, {remaining} remaining, {:.0}s elapsed)",
                t0.elapsed().as_secs_f64()
            );
            respond_json(req, 200, "{\"status\":\"ok\"}".into());
        }
        _ => respond_json(req, 404, "{\"error\":\"no such endpoint\"}".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: Duration = Duration::from_secs(100);

    #[test]
    fn leases_in_ascending_order() {
        let mut c = Coordinator::new(3, TTL);
        let now = Instant::now();
        assert_eq!(c.lease(now), Some(0));
        assert_eq!(c.lease(now), Some(1));
        assert_eq!(c.lease(now), Some(2));
        assert_eq!(c.lease(now), None, "universe exhausted while all leases active");
    }

    #[test]
    fn no_double_lease_while_active() {
        let mut c = Coordinator::new(2, TTL);
        let now = Instant::now();
        assert_eq!(c.lease(now), Some(0));
        // Well within the TTL: shard 0 must not be handed out again.
        assert_eq!(c.lease(now + Duration::from_secs(50)), Some(1));
    }

    #[test]
    fn expired_lease_requeues() {
        let mut c = Coordinator::new(2, TTL);
        let now = Instant::now();
        assert_eq!(c.lease(now), Some(0));
        // Past the TTL the lowest free shard is 0 again (dead host recovery).
        assert_eq!(c.lease(now + TTL), Some(0));
    }

    #[test]
    fn complete_clears_lease_and_is_idempotent() {
        let mut c = Coordinator::new(2, TTL);
        let now = Instant::now();
        assert_eq!(c.lease(now), Some(0));
        assert!(c.complete(0), "first completion is outstanding");
        assert!(!c.complete(0), "second completion is a duplicate");
        assert_eq!(c.counts(now), (1, 0, 1));
        // 0 never comes back, even after every lease would have expired.
        assert_eq!(c.lease(now + TTL * 2), Some(1));
    }

    #[test]
    fn late_completion_after_expiry_still_lands() {
        let mut c = Coordinator::new(2, TTL);
        let now = Instant::now();
        assert_eq!(c.lease(now), Some(0));
        assert_eq!(c.lease(now + TTL), Some(0), "re-leased after expiry");
        // The original (slow) host finishes anyway — accepted.
        assert!(c.complete(0));
        assert_eq!(c.counts(now + TTL), (1, 0, 1));
    }

    #[test]
    fn mark_done_rescan_skips_existing() {
        let mut c = Coordinator::new(3, TTL);
        c.mark_done(0);
        c.mark_done(2);
        let now = Instant::now();
        assert_eq!(c.counts(now), (2, 0, 1));
        assert_eq!(c.lease(now), Some(1));
        assert_eq!(c.lease(now), None);
    }
}
