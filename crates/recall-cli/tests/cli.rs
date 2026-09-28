//! End-to-end CLI tests: remember -> recall -> export -> import round trip
//! through the real binary, exactly as a script would drive it.

use std::process::Command;

use serde_json::Value;

struct Output {
    stdout: String,
    status: std::process::ExitStatus,
}

fn run(db: &str, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_recall-cli"))
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .expect("run recall-cli");
    Output {
        stdout: String::from_utf8(out.stdout).expect("utf8 stdout"),
        status: out.status,
    }
}

#[test]
fn cli_remember_recall_export_import_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("e2e.db");
    let db_str = db.to_str().unwrap();

    // remember (auto-creates namespace)
    let out = run(
        db_str,
        &[
            "remember",
            "--namespace",
            "e2e",
            "--text",
            "cargo test drives the cli",
            "--tags",
            "test,cli",
        ],
    );
    assert!(out.status.success(), "remember failed: {}", out.stdout);
    let memory: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(memory["text"], "cargo test drives the cli");
    assert_eq!(memory["source"], "cli");

    // recall finds it with a breakdown
    let out = run(
        db_str,
        &[
            "recall",
            "--namespace",
            "e2e",
            "--query",
            "cli test",
            "--k",
            "3",
        ],
    );
    assert!(out.status.success(), "recall failed: {}", out.stdout);
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert!(hits[0]["breakdown"]["total"].as_f64().unwrap() > 0.0);

    // export to file
    let export_path = dir.path().join("export.json");
    let out = run(db_str, &["export", "--out", export_path.to_str().unwrap()]);
    assert!(out.status.success(), "export failed: {}", out.stdout);
    assert!(export_path.exists());

    // import into a fresh db, skipping nothing
    let db2 = dir.path().join("e2e-2.db");
    let out = run(
        db2.to_str().unwrap(),
        &["import", "--file", export_path.to_str().unwrap()],
    );
    assert!(out.status.success(), "import failed: {}", out.stdout);
    let report: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(report["memories_inserted"], 1);
    assert_eq!(report["namespaces_created"], 1);

    // recall in the imported db finds the memory
    let out = run(
        db2.to_str().unwrap(),
        &["recall", "--namespace", "e2e", "--query", "cargo cli"],
    );
    assert!(out.status.success());
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);
}

/// Accessibility contract for the CLI surface: printed JSON omits the
/// embedding vector unless `--show-embedding` is passed (a screen reader
/// would otherwise announce hundreds of zeros per record).
#[test]
fn cli_output_omits_embeddings_unless_asked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("emb.db");
    let db_str = db.to_str().unwrap();

    let out = run(
        db_str,
        &[
            "remember",
            "--namespace",
            "a11y",
            "--text",
            "quiet terminal output",
        ],
    );
    assert!(out.status.success(), "remember failed: {}", out.stdout);
    let memory: Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(
        memory.get("embedding").is_none(),
        "default remember output must omit the embedding"
    );
    let id = memory["id"].as_str().unwrap().to_string();

    let out = run(
        db_str,
        &["recall", "--namespace", "a11y", "--query", "quiet output"],
    );
    assert!(out.status.success(), "recall failed: {}", out.stdout);
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert!(
        hits[0].get("embedding").is_none(),
        "default recall output must omit the embedding"
    );
    assert!(
        hits[0]["breakdown"]["total"].as_f64().unwrap() > 0.0,
        "breakdown stays"
    );

    let out = run(db_str, &["update", "--id", &id, "--pinned", "true"]);
    assert!(out.status.success(), "update failed: {}", out.stdout);
    let updated: Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(
        updated.get("embedding").is_none(),
        "default update output must omit the embedding"
    );

    let out = run(
        db_str,
        &[
            "recall",
            "--namespace",
            "a11y",
            "--query",
            "quiet output",
            "--show-embedding",
        ],
    );
    assert!(
        out.status.success(),
        "flagged recall failed: {}",
        out.stdout
    );
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(
        hits[0]["embedding"]
            .as_array()
            .is_some_and(|v| !v.is_empty()),
        "--show-embedding must restore the vector"
    );
}

#[test]
fn cli_rejects_unknown_namespace_on_strict_paths_and_bad_queries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("neg.db");

    // recall on unknown namespace errors (non-zero exit)
    let out = run(
        db.to_str().unwrap(),
        &["recall", "--namespace", "ghost", "--query", "x"],
    );
    assert!(!out.status.success());

    // blank text is rejected
    let out = run(
        db.to_str().unwrap(),
        &["remember", "--namespace", "n", "--text", "   "],
    );
    assert!(!out.status.success());
}

#[test]
fn cli_update_and_tag_filtered_recall() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("upd.db");
    let db_str = db.to_str().unwrap();

    let out = run(
        db_str,
        &[
            "remember",
            "--namespace",
            "ops",
            "--text",
            "postgres wal checkpoints every 5 minutes",
            "--tags",
            "db,postgres",
        ],
    );
    assert!(out.status.success(), "remember failed: {}", out.stdout);
    let memory: Value = serde_json::from_str(&out.stdout).unwrap();
    let id = memory["id"].as_str().unwrap().to_string();

    // Update text in place; tags replaced, id preserved.
    let out = run(
        db_str,
        &[
            "update",
            "--id",
            &id,
            "--text",
            "postgres checkpoint tuning moved to weekly review",
            "--tags",
            "db,tuning",
        ],
    );
    assert!(out.status.success(), "update failed: {}", out.stdout);
    let updated: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(updated["tags"], serde_json::json!(["db", "tuning"]));

    // Tag-filtered recall hits the updated text; the stale tag matches nothing.
    let out = run(
        db_str,
        &[
            "recall",
            "--namespace",
            "ops",
            "--query",
            "postgres checkpoints",
            "--tags",
            "db",
        ],
    );
    assert!(out.status.success(), "recall failed: {}", out.stdout);
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert!(
        hits[0]["text"]
            .as_str()
            .unwrap()
            .contains("checkpoint tuning")
    );

    let out = run(
        db_str,
        &[
            "recall",
            "--namespace",
            "ops",
            "--query",
            "postgres checkpoints",
            "--tags",
            "postgres",
        ],
    );
    assert!(out.status.success(), "recall failed: {}", out.stdout);
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 0, "old tag must be gone");
}

/// Encryption at rest (D-014) end-to-end through the real binary: with
/// `RECALL_MCP_KEY` set, the stored database file must not contain the
/// memory text, while the CLI still round-trips it; without the key the
/// database refuses to open.
#[test]
fn cli_encrypts_at_rest_when_key_is_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("enc.db");
    let db_str = db.to_str().unwrap();
    const SECRET: &str = "the orchard hides the launch codes";

    let run_keyed = |db: &str, args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_recall-cli"))
            .arg("--db")
            .arg(db)
            .args(args)
            .env("RECALL_MCP_KEY", KEY)
            .output()
            .expect("run recall-cli");
        Output {
            stdout: String::from_utf8(out.stdout).expect("utf8 stdout"),
            status: out.status,
        }
    };
    const KEY: &str = "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a";

    let out = run_keyed(
        db_str,
        &["remember", "--namespace", "sec", "--text", SECRET],
    );
    assert!(
        out.status.success(),
        "keyed remember failed: {}",
        out.stdout
    );
    let memory: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(memory["text"], SECRET, "API returns plaintext");

    // The database file itself must not contain the secret or its tokens.
    let db_bytes = std::fs::read(&db).expect("read db");
    let raw = String::from_utf8_lossy(&db_bytes);
    assert!(!raw.contains("orchard"), "plaintext leaked to disk");
    assert!(!raw.contains("launch"), "plaintext leaked to disk");

    // Keyed recall decrypts and ranks.
    let out = run_keyed(
        db_str,
        &["recall", "--namespace", "sec", "--query", "orchard"],
    );
    assert!(out.status.success(), "keyed recall failed: {}", out.stdout);
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(hits[0]["text"], SECRET);

    // Without the key, opening the encrypted database must fail (export reads it).
    let out = run(db_str, &["export"]);
    assert!(!out.status.success(), "unkeyed open must be refused");
}

#[test]
fn cli_backup_and_vacuum_maintain_the_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("maint.db");
    let db_str = db.to_str().unwrap();
    let backup = dir.path().join("snapshot.db");
    let backup_str = backup.to_str().unwrap();

    let out = run(
        db_str,
        &[
            "remember",
            "--namespace",
            "maint",
            "--text",
            "backup this observation",
        ],
    );
    assert!(out.status.success(), "remember failed: {}", out.stdout);

    // Hot backup into a new file; reports counts as JSON.
    let out = run(db_str, &["backup", "--out", backup_str]);
    assert!(out.status.success(), "backup failed: {}", out.stdout);
    let report: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(report["total_memories"], 1);
    assert_eq!(report["total_namespaces"], 1);
    assert!(backup.exists());

    // Refuses to overwrite an existing target.
    let out = run(db_str, &["backup", "--out", backup_str]);
    assert!(!out.status.success(), "backup must not overwrite");

    // The backup is a working store: recall against it succeeds.
    let out = run(
        backup_str,
        &[
            "recall",
            "--namespace",
            "maint",
            "--query",
            "backup observation",
        ],
    );
    assert!(
        out.status.success(),
        "recall on backup failed: {}",
        out.stdout
    );
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);

    // In-place vacuum works and keeps the data.
    let out = run(db_str, &["vacuum"]);
    assert!(out.status.success(), "vacuum failed: {}", out.stdout);
    let report: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(report["total_memories"], 1);
    let out = run(
        db_str,
        &[
            "recall",
            "--namespace",
            "maint",
            "--query",
            "backup observation",
        ],
    );
    assert!(
        out.status.success(),
        "post-vacuum recall failed: {}",
        out.stdout
    );
}

/// `--embedder` selection end-to-end through the real binary: unknown names
/// fail the command (serve included), the default `hash` path works, and the
/// same flag drives both writing and reading.
#[test]
fn cli_embedder_flag_selects_and_validates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("emb.db");
    let db_str = db.to_str().unwrap();

    // Unknown embedder names fail one-shot commands.
    let out = run(
        db_str,
        &[
            "remember",
            "--embedder",
            "gpt4",
            "--namespace",
            "e",
            "--text",
            "x",
        ],
    );
    assert!(
        !out.status.success(),
        "unknown embedder must fail the command"
    );

    // ...and fail `serve` during config resolution, before it can bind.
    let out = run(db_str, &["serve", "--embedder", "gpt4"]);
    assert!(!out.status.success(), "serve must fail closed");

    // The explicit default works through the same selection path.
    let out = run(
        db_str,
        &[
            "remember",
            "--embedder",
            "hash",
            "--namespace",
            "e",
            "--text",
            "embedded via the explicit embedder flag",
        ],
    );
    assert!(out.status.success(), "remember failed: {}", out.stdout);

    let out = run(
        db_str,
        &[
            "recall",
            "--embedder",
            "hash",
            "--namespace",
            "e",
            "--query",
            "explicit embedder flag",
        ],
    );
    assert!(out.status.success(), "recall failed: {}", out.stdout);
    let hits: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Process-level serve glue: a `--config` file drives a REAL server over real
// sockets. The unit tests in main.rs stop at `ServerConfig`; this proves the
// resolved config actually reaches the running router (bind from file, body
// limit 413, rate-limit 429 with Retry-After, probes unlimited).
// ---------------------------------------------------------------------------

struct ServerGuard {
    child: std::process::Child,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One raw HTTP/1.1 request over its own connection (`Connection: close`).
/// Returns (status code, all header lines lower-cased, body).
fn http_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Vec<String>, String) {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(20)))
        .expect("write timeout");
    let body_bytes = body.unwrap_or("").as_bytes();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );
    use std::io::{Read, Write};
    stream.write_all(request.as_bytes()).expect("write head");
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes).expect("write body");
    }
    // No client half-close here: hyper treats an early client FIN as
    // end-of-stream and answers with an empty connection. The request asks
    // for `Connection: close`, so the server's own FIN ends `read_to_end`.
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    let text = String::from_utf8_lossy(&raw).to_string();
    let mut parts = text.splitn(2, "\r\n\r\n");
    let head = parts.next().unwrap_or("");
    let body = parts.next().unwrap_or("").to_string();
    let mut head_lines = head.split("\r\n");
    let status_line = head_lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status line in response: {text:.200}"));
    let headers = head_lines.map(|h| h.to_ascii_lowercase()).collect();
    (status, headers, body)
}

#[test]
fn cli_serve_applies_a_config_file_over_real_http() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("serve-glue.db");

    // Grab a free port, then hand it to the server *via the config file* —
    // the bind address itself is part of the glue under test.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("probe port");
    let port = listener.local_addr().expect("port").port();
    drop(listener);

    // rate_limit 1 -> burst default max(2x1, 10) = 10; body_limit 512 bytes.
    let config_path = dir.path().join("serve.json");
    std::fs::write(
        &config_path,
        format!(
            r#"{{"bind": "127.0.0.1:{port}", "rate_limit": 1, "body_limit": 512, "embedder": "hash"}}"#
        ),
    )
    .expect("write config");

    let child = Command::new(env!("CARGO_BIN_EXE_recall-cli"))
        .arg("--db")
        .arg(&db)
        .args(["serve", "--config", config_path.to_str().unwrap()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn recall-cli serve");
    let _server = ServerGuard { child };

    // Wait for /healthz (the probe path is deliberately unlimited).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut up = false;
    while std::time::Instant::now() < deadline {
        if let Ok(stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            drop(stream);
            let (status, _, _) = http_request(port, "GET", "/healthz", None);
            if status == 200 {
                up = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(
        up,
        "server never became healthy on the file-configured bind"
    );

    // Sanity: the protected API works through the same spawn (this also
    // spends 2 rate-limit tokens: namespace create + memory create).
    let payload = r#"{"name":"glue"}"#;
    let (status, _, _) = http_request(port, "POST", "/v1/namespaces", Some(payload));
    assert_eq!(status, 201, "namespace create must succeed");
    let payload = r#"{"namespace":"glue","text":"config glue end to end"}"#;
    let (status, _, _) = http_request(port, "POST", "/v1/memories", Some(payload));
    assert_eq!(status, 201, "memory create must succeed");

    // The file's 512-byte body limit engages: a 700-byte body is 413.
    let big = format!(r#"{{"namespace":"glue","text":"{}"}}"#, "x".repeat(700));
    let (status, _, _) = http_request(port, "POST", "/v1/memories", Some(&big));
    assert_eq!(status, 413, "a body over the file's limit must be 413");

    // The file's rate limit engages: burst 10 was spent 3 deep by now, so a
    // rapid flood of 15 must produce 429s, each carrying Retry-After.
    let mut saw_429 = false;
    let mut last_status = 0;
    for _ in 0..15 {
        let (status, headers, _) = http_request(port, "GET", "/v1/namespaces", None);
        last_status = status;
        if status == 429 {
            saw_429 = true;
            assert!(
                headers.iter().any(|h| h.starts_with("retry-after:")),
                "every 429 must carry Retry-After"
            );
        }
    }
    assert!(saw_429, "the flood must hit the rate limit at least once");
    assert_eq!(
        last_status, 429,
        "the sustained rate (1/s) cannot refill within the flood"
    );

    // Probes stay unlimited even with an empty bucket (D-017).
    let (status, _, _) = http_request(port, "GET", "/healthz", None);
    assert_eq!(status, 200, "healthz must never be rate limited");
}
