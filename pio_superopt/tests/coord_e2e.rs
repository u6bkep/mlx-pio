//! End-to-end test of the shard coordinator: the REAL HTTP server loop
//! (`coord::serve` on an ephemeral port, same code the `superopt serve`
//! subcommand runs) driven by real worker-side HTTP calls (ureq, same crate
//! the `superopt work` subcommand uses). len=2 keeps it fast: 145 shards, a
//! couple run through the real `run_shard`, the rest fake-completed through
//! the protocol (the server only validates JSON + the shard field).

use pio_superopt::coord::{serve, Coordinator, ServeCfg};
use pio_superopt::enumerate::{alphabet, run_shard};
use std::time::Duration;

fn fresh_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pio-coord-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Bind on port 0, run the serve loop in a detached thread, return base URL.
fn spawn_server(len: usize, out_dir: std::path::PathBuf, pre_done: &[usize]) -> (String, usize) {
    let ops_len = alphabet(len).len();
    let mut coord = Coordinator::new(ops_len, Duration::from_secs(3600));
    for &s in pre_done {
        coord.mark_done(s);
    }
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        serve(&server, &mut coord, &ServeCfg { len, alphabet: ops_len, out_dir });
    });
    (format!("http://127.0.0.1:{port}"), ops_len)
}

fn get_status(base: &str) -> serde_json::Value {
    let body = ureq::get(&format!("{base}/status")).call().unwrap().into_string().unwrap();
    serde_json::from_str(&body).unwrap()
}

#[test]
fn coordinator_end_to_end() {
    let len = 2usize;
    let out = fresh_dir("main");
    let (base, total) = spawn_server(len, out.clone(), &[]);
    let ops = alphabet(len);
    assert_eq!(ops.len(), total);

    // Contract check: a worker with the wrong alphabet size is refused.
    let err = ureq::post(&format!("{base}/lease"))
        .send_string(&format!("{{\"len\":{len},\"alphabet\":{}}}", total + 1))
        .expect_err("mismatched alphabet must be rejected");
    match err {
        ureq::Error::Status(code, resp) => {
            assert_eq!(code, 409);
            let msg = resp.into_string().unwrap();
            assert!(msg.contains("contract mismatch"), "unhelpful 409 body: {msg}");
        }
        other => panic!("expected status error, got {other}"),
    }

    // Status before any work.
    let st = get_status(&base);
    assert_eq!(st["len"].as_u64().unwrap() as usize, len);
    assert_eq!(st["alphabet"].as_u64().unwrap() as usize, total);
    assert_eq!(st["done"].as_u64().unwrap(), 0);
    assert_eq!(st["remaining"].as_u64().unwrap() as usize, total);

    // Drain the whole universe through lease/done. The first two shards go
    // through the real run_shard; the rest post minimal fake results (the
    // protocol only validates JSON + the shard field).
    let lease_body = format!("{{\"len\":{len},\"alphabet\":{total}}}");
    let mut real_result_0: Option<String> = None;
    let mut leased = Vec::new();
    loop {
        let resp = ureq::post(&format!("{base}/lease")).send_string(&lease_body).unwrap();
        if resp.status() == 204 {
            break; // drained
        }
        let v: serde_json::Value = serde_json::from_str(&resp.into_string().unwrap()).unwrap();
        let shard = v["shard"].as_u64().unwrap() as usize;
        leased.push(shard);
        let body = if shard < 2 {
            let res = run_shard(shard, len, &ops);
            assert!(res.structures > 0, "real shard ran zero structures");
            let s = serde_json::to_string_pretty(&res).unwrap();
            if shard == 0 {
                real_result_0 = Some(s.clone());
            }
            s
        } else {
            format!("{{\"shard\":{shard},\"len\":{len},\"alphabet\":{total},\"fake\":true}}")
        };
        let done = ureq::post(&format!("{base}/done?shard={shard}")).send_string(&body).unwrap();
        assert_eq!(done.status(), 200);
    }

    // Every shard was leased exactly once and everything ended done.
    let mut sorted = leased.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), leased.len(), "a shard was double-leased");
    assert_eq!(sorted, (0..total).collect::<Vec<_>>(), "not every shard was leased");
    let st = get_status(&base);
    assert_eq!(st["done"].as_u64().unwrap() as usize, total);
    assert_eq!(st["leased"].as_u64().unwrap(), 0);
    assert_eq!(st["remaining"].as_u64().unwrap(), 0);

    // Shard files exist and parse, with the right shard field.
    for shard in 0..total {
        let path = out.join(format!("shard-{shard:04}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["shard"].as_u64().unwrap() as usize, shard);
    }

    // Double-done is idempotent: 200, and the shard stays done.
    let dup = ureq::post(&format!("{base}/done?shard=0"))
        .send_string(&real_result_0.expect("shard 0 went through run_shard"))
        .unwrap();
    assert_eq!(dup.status(), 200);
    let st = get_status(&base);
    assert_eq!(st["done"].as_u64().unwrap() as usize, total);

    // Done body whose shard field disagrees with the query is refused.
    let err = ureq::post(&format!("{base}/done?shard=1"))
        .send_string("{\"shard\":2}")
        .expect_err("shard mismatch must be rejected");
    match err {
        ureq::Error::Status(code, _) => assert_eq!(code, 400),
        other => panic!("expected status error, got {other}"),
    }

    std::fs::remove_dir_all(&out).ok();
}

/// Startup rescan (mark_done before serving) keeps pre-done shards out of
/// the lease stream — the serve subcommand's resume path.
#[test]
fn rescan_skips_pre_done_shards() {
    let len = 2usize;
    let out = fresh_dir("rescan");
    let (base, total) = spawn_server(len, out.clone(), &[0, 3]);

    let st = get_status(&base);
    assert_eq!(st["done"].as_u64().unwrap(), 2);
    assert_eq!(st["remaining"].as_u64().unwrap() as usize, total - 2);

    let lease_body = format!("{{\"len\":{len},\"alphabet\":{total}}}");
    let mut first = Vec::new();
    for _ in 0..3 {
        let resp = ureq::post(&format!("{base}/lease")).send_string(&lease_body).unwrap();
        let v: serde_json::Value = serde_json::from_str(&resp.into_string().unwrap()).unwrap();
        first.push(v["shard"].as_u64().unwrap() as usize);
    }
    assert_eq!(first, vec![1, 2, 4], "pre-done shards 0 and 3 must be skipped, in order");

    std::fs::remove_dir_all(&out).ok();
}
