//! Publish a depth batch the way a tickerplant does, and read it back.
//!
//! ```sh
//! rayforce -p 5010 &
//! cargo run --example depth
//! ```
//!
//! The server must have an `.ipc.on.async` hook, or the batch is evaluated as a function call and
//! answered with an error. The hook this example installs stores the message it is handed, which
//! is the shape the tests use too.

use rayforce_ipc::{Conn, Sym, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("RAYFORCE_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(5010);
    let mut conn = Conn::connect("127.0.0.1", port)?;

    // `set` answers with the value it bound, and a lambda is not a value this client decodes, so
    // the expression ends in `null`.
    conn.send_sync(&Value::Str(
        "(do (set .ipc.on.async (fn [m] (set last_upd m))) null)".into(),
    ))?;

    let instruments = ["BTCUSDT", "ETHUSDT", "BTCUSDT", "SOLUSDT"];
    let batch = Value::table([
        (
            "ts_evt",
            Value::Timestamps(vec![
                843_048_000_500_000_000,
                843_048_001_500_000_000,
                843_048_002_500_000_000,
                843_048_003_500_000_000,
            ]),
        ),
        (
            "instrument",
            Value::Syms(instruments.map(Sym::intern).to_vec()),
        ),
        (
            "px",
            Value::I64s(vec![
                1_000_000_000,
                2_000_000_000,
                3_000_000_000,
                4_000_000_000,
            ]),
        ),
        ("price", Value::F64s(vec![1.5, 2.5, 3.5, 4.5])),
        ("f_last", Value::Bools(vec![true, false, true, false])),
    ])?;

    conn.send_async(&Value::List(vec![
        Value::Sym(Sym::intern("upd")),
        Value::Sym(Sym::intern("depth")),
        batch,
    ]))?;

    // An async message has no reply; a sync call is the barrier that proves the server processed
    // it, because the server answers in order.
    let rows = conn.send_sync(&Value::Str("(count (at last_upd 2))".into()))?;
    let symbols = conn.send_sync(&Value::Str("(cols (at last_upd 2))".into()))?;
    let prices = conn.send_sync(&Value::Str("(get (at last_upd 2) 'price)".into()))?;
    let stamp = conn.send_sync(&Value::Str(
        "(as 'STR (first (get (at last_upd 2) 'ts_evt)))".into(),
    ))?;

    println!("rows:    {rows:?}");
    println!("columns: {symbols:?}");
    println!("prices:  {prices:?}");
    println!("first:   {stamp:?}");
    Ok(())
}
