# rayforce-rs-ipc

RayforceDB's IPC protocol in Rust, written against the wire format rather than against
`librayforce`. No C stubs, no embedded VM, no symbol table shared with a server, no dependencies,
and nothing in the process that cares which thread it runs on.

```rust
use rayforce_ipc::{Conn, Sym, Value};

let mut conn = Conn::connect("127.0.0.1", 5010)?;
let batch = Value::table([
    ("instrument", Value::Syms(vec![Sym::intern("BTCUSDT"), Sym::intern("ETHUSDT")])),
    ("px", Value::F64s(vec![83_592.01, 2_543.56])),
])?;
conn.send_async(&Value::List(vec![
    Value::Sym(Sym::intern("upd")),
    Value::Sym(Sym::intern("depth")),
    batch,
]))?;
```

This is a Rust port of the OCaml client in
[`ocaml-rayforce-ipc`](https://github.com/deepmarker/ocaml-rayforce-ipc), by the same author.

## What is here

| Module | What it is |
|---|---|
| `sym` | Local intern table. A symbol travels as a string, so an id is ours alone — it exists so a column holds one int per row. |
| `value` | The value model, sized and filled straight into a `Vec<u8>`. A column is an ordinary `Vec`. |
| `wire` | The 16-byte frame header, and the delta+RLE decoder for a compressed frame. |
| `conn` | A blocking client: connect, handshake, `send_async`, `send_sync`. |
| `column` | A column built a row at a time, already in wire layout. |
| `temporal` | Checked conversions between Unix time and Rayforce's 2000-01-01 epoch. |

## The format

Wire version 4, as of rayforce v3.0.2. Version 4 adds the error message to an error record (code,
one length byte, message); a peer on version 3 is refused at the handshake, as the server itself
refuses it. The authority is `src/store/serde.{h,c}` and `src/core/ipc.{h,c}` in the rayforce tree.

- **Handshake** — the client sends `{4, 0}`; the server answers `{version, auth_required}`, and if
  it asked for credentials, one length byte plus `user:password\0` and a one-byte verdict.
- **Frame** — `prefix 0xcefadefa`, version, flags, endian, message type, and an `i64` payload
  length. The payload is native-endian, and a peer whose byte order differs refuses the frame
  rather than reading columns backwards.
- **Values** — a vector is `type, attrs, i64 length, payload`; an atom is `type, flags, value`
  with the type negated. A table is a symbol vector of column names followed by a list of columns.
- **Symbols** are NUL-terminated strings on the wire. The id a server hands out indexes its own
  intern table, which is why `Sym` can keep its own and why two processes need no coordination to
  agree on a name.
- **Compression** is a per-frame, sender-side decision with nothing to negotiate, so this client
  never compresses and always decompresses. Rayforce never compresses to a loopback or
  UNIX-domain peer anyway.

A wire-version bump is a connect-time error here, not a silent misparse — the handshake compares
versions before anything is sent.

## Which client

There is one: the blocking `Conn`. It is deliberately blocking, because the callers it was written
for send from a polling loop that owns its own thread, and a batch is one write of a buffer they
have already built. A send on a connection whose peer has stopped draining will stall that thread.
An async client with a buffered writer and the urgent-byte cancel is the next thing to port; the
OCaml crate has both and this one does not yet.

## Differences from the OCaml client

- `Sym` is process-wide and thread-safe (a `RwLock` around the intern table) instead of being
  single-threaded by assumption. A symbol interned on one thread is usable on another.
- Columns hold `i64` cells throughout. The OCaml `Column.add_int` is 63-bit and its temporal tests
  use `add_int64` for the extremes; here there is one integer width, so that trap is gone.
- `temporal` works in `i64` nanoseconds rather than `Core.Time_ns` / `Time_ns.Span`. The epoch
  offset, the null (`i64::MIN`) and the checked arithmetic are the same, and so are the two
  overflow cases: a value near either end of the range is reported, never wrapped.
- The OCaml crate's temporal helpers raise on the int API; here every conversion returns
  `Result` and nothing panics on an unrepresentable instant.
- No async client, no cancel byte, no shared-memory or RDMA transport negotiation. A connection
  stays on TCP, which is what a peer that cannot upgrade would use anyway.

## Tests

```sh
cargo test
```

Unit and golden-byte tests need nothing. The live test — `tests/live.rs` — starts a throwaway
`rayforce -p <port>` on a port derived from the test process's id, waits for the listener, and
checks values this client encoded against Rayforce's own evaluator. It needs `rayforce` on `PATH`;
when the binary is absent the test says so and passes, because a client library should not fail a
build on a missing server.

The live test covers the handshake, sync evaluation, the async `.ipc.on.async` hook with a
tickerplant-shaped batch, the epoch rendered by the server's own calendar, timespan precision
above 2^53, typed nulls, a 5,000-row batch past the compression threshold, and an error reply
carrying the server's code and message.

## Licence

MIT. Copyright (c) 2026 Vincent Bernardoff.
