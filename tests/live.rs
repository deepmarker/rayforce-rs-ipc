//! The client against a real server.
//!
//! A round trip through our own decoder proves only that we are self-consistent. What matters is
//! that the other end agrees, so every check here is a value this client encoded, read back
//! through Rayforce's own evaluator. `rayforce` must be on `PATH`; when it is not, the test says
//! so and passes, because a client library should not fail a build on a missing server.

use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rayforce_ipc::{Conn, Sym, Value};

/// A throwaway server, killed and reaped when the test ends.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start `rayforce -p <port>` and wait for it to accept connections.
///
/// The port comes from this process's id so parallel runs on one machine do not collide, and
/// each caller passes a different `offset` so two tests in one binary get a server each: sharing
/// one would mean whichever test finished first killed the other's.
fn server(offset: u16) -> Option<Server> {
    let port = 16_000 + (std::process::id() % 500) as u16 * 2 + offset;
    let child = Command::new("rayforce")
        .arg("-p")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut server = Server { child, port };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        // A server that has already exited lost the port to someone else; say so rather than
        // spinning for ten seconds.
        if let Ok(Some(_)) = server.child.try_wait() {
            return None;
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(server);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn eval(conn: &mut Conn, source: &str) -> Value {
    conn.send_sync(&Value::Str(source.into()))
        .unwrap_or_else(|error| panic!("{source}: {error}"))
}

fn check(conn: &mut Conn, source: &str, expected: Value) {
    let got = eval(conn, source);
    assert_eq!(got, expected, "{source}");
}

/// The batch the tickerplant example sends: one table, five columns, four rows.
fn depth_batch(rows: usize) -> Value {
    let ts_evt: Vec<i64> = (0..rows)
        .map(|i| 843_048_000_500_000_000 + 1_000_000_000 * i as i64)
        .collect();
    let instrument = ["BTCUSDT", "ETHUSDT", "BTCUSDT", "SOLUSDT"]
        .iter()
        .take(rows)
        .map(|name| Sym::intern(name))
        .collect();
    let px: Vec<i64> = (0..rows).map(|i| (i as i64 + 1) * 1_000_000_000).collect();
    let price: Vec<f64> = (0..rows).map(|i| 1.5 + i as f64).collect();
    let f_last: Vec<bool> = (0..rows).map(|i| i % 2 == 0).collect();
    Value::table([
        ("ts_evt", Value::Timestamps(ts_evt)),
        ("instrument", Value::Syms(instrument)),
        ("px", Value::I64s(px)),
        ("price", Value::F64s(price)),
        ("f_last", Value::Bools(f_last)),
    ])
    .expect("table")
}

#[test]
fn a_request_can_carry_its_own_deadline() {
    let Some(server) = server(2) else {
        eprintln!("skipping the live test: rayforce is not on PATH");
        return;
    };
    let mut conn = Conn::connect("127.0.0.1", server.port).expect("connect");
    let answer = conn
        .send_sync_timeout(&Value::Str("(+ 40 2)".into()), Duration::from_secs(5))
        .expect("request with a deadline");
    assert_eq!(answer, Value::I64(42));
    // The deadline is per request: the next one passes its own.
    let answer = conn
        .send_sync_timeout(&Value::Str("(+ 1 1)".into()), Duration::from_secs(5))
        .expect("second request");
    assert_eq!(answer, Value::I64(2));
    assert_eq!(
        conn.send_sync_timeout(&Value::Null, Duration::ZERO)
            .unwrap_err()
            .to_string(),
        "a send timeout must be greater than zero"
    );
}

#[test]
fn encode_and_read_back() {
    let Some(server) = server(0) else {
        eprintln!("skipping the live test: rayforce is not on PATH");
        return;
    };
    let mut conn = Conn::connect("127.0.0.1", server.port).expect("connect");

    // Sync: the server parses and evaluates the string we sent.
    check(&mut conn, "(+ 1 2)", Value::I64(3));
    check(&mut conn, "(til 4)", Value::I64s(vec![0, 1, 2, 3]));

    // Async: an ingest hook of the shape the tickerplant uses, so the batch is stored under the
    // name the message carries rather than evaluated. The `null` is not decoration: `set`
    // answers with the value it bound, and a lambda is not a value this client decodes.
    check(
        &mut conn,
        "(do (set .ipc.on.async (fn [m] (set last_upd m))) null)",
        Value::Null,
    );
    conn.send_async(&Value::List(vec![
        Value::Sym(Sym::intern("upd")),
        Value::Sym(Sym::intern("depth")),
        depth_batch(4),
    ]))
    .expect("send_async");

    // Async has no reply to wait on, and the hook runs on the server's own loop, so read the
    // result back with sync calls: the server can only answer them after it has processed what
    // came before.
    check(&mut conn, "(at last_upd 0)", Value::Sym(Sym::intern("upd")));
    check(
        &mut conn,
        "(at last_upd 1)",
        Value::Sym(Sym::intern("depth")),
    );
    check(&mut conn, "(count (at last_upd 2))", Value::I64(4));
    check(
        &mut conn,
        "(cols (at last_upd 2))",
        Value::Syms(
            ["ts_evt", "instrument", "px", "price", "f_last"]
                .map(Sym::intern)
                .to_vec(),
        ),
    );
    check(
        &mut conn,
        "(get (at last_upd 2) 'instrument)",
        Value::Syms(
            ["BTCUSDT", "ETHUSDT", "BTCUSDT", "SOLUSDT"]
                .map(Sym::intern)
                .to_vec(),
        ),
    );
    check(
        &mut conn,
        "(get (at last_upd 2) 'px)",
        Value::I64s(vec![
            1_000_000_000,
            2_000_000_000,
            3_000_000_000,
            4_000_000_000,
        ]),
    );
    check(
        &mut conn,
        "(get (at last_upd 2) 'price)",
        Value::F64s(vec![1.5, 2.5, 3.5, 4.5]),
    );
    check(
        &mut conn,
        "(get (at last_upd 2) 'f_last)",
        Value::Bools(vec![true, false, true, false]),
    );

    // The epoch offset is the easiest thing in the format to get wrong, and a server that
    // disagrees says so in its own calendar.
    check(
        &mut conn,
        "(as 'STR (first (get (at last_upd 2) 'ts_evt)))",
        Value::Str("2026.09.18D12:00:00.500000000".into()),
    );

    // Timespan atoms and vectors through the engine's evaluator and serializer.
    check(&mut conn, "0D00:00:00.000000001", Value::Timespan(1));
    check(&mut conn, "-0D00:00:00.000000001", Value::Timespan(-1));
    check(&mut conn, "1D", Value::Timespan(86_400_000_000_000));

    // 2^53 + 1 is not representable as an f64: anything on the float path loses it.
    check(
        &mut conn,
        "(do (set temporal_echo (fn [x] x)) null)",
        Value::Null,
    );
    assert_eq!(
        conn.send_sync(&Value::List(vec![
            Value::Sym(Sym::intern("temporal_echo")),
            Value::Timespan(9_007_199_254_740_993),
        ]))
        .expect("timespan precision"),
        Value::Timespan(9_007_199_254_740_993)
    );

    let spans = vec![
        0_i64,
        -1,
        86_400_000_000_000,
        9_007_199_254_740_993,
        i64::MIN,
    ];
    assert_eq!(
        conn.send_sync(&Value::List(vec![
            Value::Sym(Sym::intern("temporal_echo")),
            Value::Timespans(spans.clone()),
        ]))
        .expect("timespan vector"),
        Value::Timespans(spans.clone())
    );
    assert_eq!(
        conn.send_sync(&Value::List(vec![
            Value::Sym(Sym::intern("nil?")),
            Value::Timespan(i64::MIN),
        ]))
        .expect("null timespan"),
        Value::Bool(true)
    );
    assert_eq!(
        conn.send_sync(&Value::List(vec![
            Value::Sym(Sym::intern("temporal_echo")),
            Value::Timespans(Vec::new()),
        ]))
        .expect("empty timespan vector"),
        Value::Timespans(Vec::new())
    );

    // A table whose timespan column was built a row at a time, read back by name.
    let mut column = rayforce_ipc::Column::timespan(spans_len(&spans));
    for span in &spans {
        column.add_i64(*span);
    }
    let echoed = conn
        .send_sync(&Value::List(vec![
            Value::Sym(Sym::intern("temporal_echo")),
            Value::table([("span", Value::Col(column))]).expect("table"),
        ]))
        .expect("timespan column");
    assert_eq!(
        echoed.column("span").ok().flatten(),
        Some(&Value::Timespans(spans.clone())),
        "the engine re-serialised the column as a vector"
    );

    // The types the engine normalises on the way out.
    check(
        &mut conn,
        "0Nl",
        Value::AtomFlags(1, Box::new(Value::I64(i64::MIN))),
    );

    // A function comes back as whatever the engine calls it, and must echo unchanged.
    let function = eval(&mut conn, "(fn [x] x)");
    assert!(
        matches!(
            function,
            Value::Lambda { .. } | Value::Unary(_) | Value::Binary(_) | Value::Variadic(_)
        ),
        "function response was {}",
        function.kind()
    );
    conn.send_async(&Value::List(vec![
        Value::Sym(Sym::intern("upd")),
        Value::Sym(Sym::intern("fn")),
        function.clone(),
    ]))
    .expect("echo the function");
    check(
        &mut conn,
        "last_upd",
        Value::List(vec![
            Value::Sym(Sym::intern("upd")),
            Value::Sym(Sym::intern("fn")),
            function,
        ]),
    );

    // A batch past the 2,000-byte compression threshold, to prove the size headroom and that
    // nothing in the frame depends on being small.
    let big_i: Vec<i64> = (0..5_000).collect();
    let big_s: Vec<Sym> = (0..5_000)
        .map(|i| Sym::intern(&format!("s{}", i % 97)))
        .collect();
    conn.send_async(&Value::List(vec![
        Value::Sym(Sym::intern("upd")),
        Value::Sym(Sym::intern("big")),
        Value::table([("i", Value::I64s(big_i)), ("s", Value::Syms(big_s))]).expect("table"),
    ]))
    .expect("send the big batch");
    check(&mut conn, "(count (at last_upd 2))", Value::I64(5_000));
    check(
        &mut conn,
        "(sum (get (at last_upd 2) 'i))",
        Value::I64(12_497_500),
    );

    // An error reply carries the server's code and text: version 4 puts the message on the wire.
    match eval(&mut conn, "(+ 1 'a)") {
        Value::Error { code, message } => {
            assert_eq!(code, "type", "error code");
            assert!(
                message.contains("cannot add"),
                "error message was {message:?}"
            );
        }
        other => panic!("expected an error value, got {}", other.kind()),
    }

    // The connection is still usable after an error reply.
    check(&mut conn, "(+ 2 2)", Value::I64(4));

    conn.close();
    assert!(conn.is_closed());
    assert_eq!(
        conn.send_async(&Value::Null).unwrap_err().to_string(),
        "the connection is closed"
    );
}

fn spans_len(spans: &[i64]) -> usize {
    spans.len()
}

#[test]
fn a_connection_opens_and_closes() {
    let Some(server) = server(1) else {
        eprintln!("skipping the live test: rayforce is not on PATH");
        return;
    };
    // Connecting to a server that wants no credentials still round-trips the version byte, and
    // the connection it leaves behind is usable.
    let mut conn = Conn::connect("127.0.0.1", server.port).expect("connect");
    assert!(!conn.is_closed());
    conn.close();
    assert!(conn.is_closed());
}
