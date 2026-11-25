//! Timestamps and timespans: the epoch, the null, and the two overflow cases.
//!
//! These mirror the OCaml client's temporal tests, transposed from `Core.Time_ns` to plain
//! `i64` nanoseconds — the difference that matters is the one both languages share: a value near
//! either end of the range must be reported, never wrapped.

use rayforce_ipc::temporal::{
    duration_to_timespan, from_unix_nanos, from_unix_seconds, timespan_to_duration, to_unix_nanos,
    EPOCH_OFFSET_NANOS, EPOCH_OFFSET_SECONDS,
};
use rayforce_ipc::{Sym, Value};

fn round_trip(value: &Value) -> Value {
    let mut buf = Vec::new();
    value.fill(&mut buf).expect("encode");
    assert_eq!(buf.len(), value.size());
    let mut at = 0;
    let decoded = Value::consume(&buf, &mut at).expect("decode");
    assert_eq!(at, buf.len());
    decoded
}

#[test]
fn timespans_travel_raw_including_both_ends() {
    for nanos in [
        0_i64,
        1,
        -1,
        86_400_000_000_000,
        9_007_199_254_740_993,
        i64::MIN,
        i64::MAX,
    ] {
        let value = Value::Timespan(nanos);
        assert_eq!(round_trip(&value), value, "timespan {nanos}");
    }
    let spans = vec![0_i64, -1, 9_007_199_254_740_993, i64::MIN, i64::MAX];
    assert_eq!(
        round_trip(&Value::Timespans(spans.clone())),
        Value::Timespans(spans)
    );
    assert_eq!(
        round_trip(&Value::Timespans(Vec::new())),
        Value::Timespans(Vec::new())
    );
}

#[test]
fn a_null_timespan_keeps_its_sentinel_and_its_zero_flags() {
    let mut buf = Vec::new();
    Value::Timespan(i64::MIN).fill(&mut buf).unwrap();
    assert_eq!(
        buf,
        [0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80],
        "the null is the payload, not a flag"
    );
}

#[test]
fn a_timespan_column_writes_the_same_bytes_as_the_vector() {
    let spans = vec![
        0_i64,
        -1,
        86_400_000_000_000,
        9_007_199_254_740_993,
        i64::MIN,
    ];
    let mut column = rayforce_ipc::Column::timespan(spans.len());
    for span in &spans {
        column.add_i64(*span);
    }
    let mut from_column = Vec::new();
    Value::Col(column).fill(&mut from_column).unwrap();
    let mut from_vector = Vec::new();
    Value::Timespans(spans.clone())
        .fill(&mut from_vector)
        .unwrap();
    assert_eq!(from_column, from_vector);
    assert_eq!(
        round_trip(&Value::Timespans(spans.clone())),
        Value::Timespans(spans)
    );
}

#[test]
fn a_table_column_of_timespans_survives_the_round_trip() {
    let spans = vec![0_i64, -1, 9_007_199_254_740_993];
    let table = Value::table([("duration", Value::Timespans(spans.clone()))]).unwrap();
    let decoded = round_trip(&table);
    assert_eq!(
        decoded.column("duration").unwrap(),
        Some(&Value::Timespans(spans))
    );
    assert_eq!(
        decoded.column("duration").unwrap().map(Value::kind),
        Some("Timespans")
    );
}

#[test]
fn the_epoch_is_2000_01_01() {
    assert_eq!(EPOCH_OFFSET_NANOS, 946_684_800_000_000_000);
    assert_eq!(EPOCH_OFFSET_SECONDS, 946_684_800);
    assert_eq!(from_unix_nanos(EPOCH_OFFSET_NANOS), Ok(0));
    assert_eq!(to_unix_nanos(0), Ok(EPOCH_OFFSET_NANOS));
}

#[test]
fn a_nineteenth_century_instant_is_representable() {
    // 1830-01-01T00:00:00Z, the case the OCaml suite pins the epoch with.
    let historic = -4_417_977_600_000_000_000;
    let rayforce = from_unix_nanos(historic).unwrap();
    assert_eq!(rayforce, -5_364_662_400_000_000_000);
    assert_eq!(to_unix_nanos(rayforce), Ok(historic));
}

#[test]
fn overflow_is_reported_not_wrapped() {
    assert!(to_unix_nanos(i64::MAX).is_err());
    assert!(from_unix_nanos(i64::MIN).is_err());
    assert!(from_unix_seconds(i64::MAX).is_err());
    assert_eq!(
        to_unix_nanos(i64::MIN).unwrap_err().to_string(),
        "null timestamp"
    );
}

#[test]
fn nanosecond_precision_above_two_to_the_fifty_third_survives() {
    let nanos = 9_007_199_254_740_993;
    let rayforce = from_unix_nanos(nanos).unwrap();
    assert_eq!(to_unix_nanos(rayforce), Ok(nanos));
    assert_eq!(
        duration_to_timespan(std::time::Duration::from_nanos(nanos as u64)),
        Ok(nanos)
    );
    assert_eq!(
        timespan_to_duration(nanos),
        Ok(std::time::Duration::from_nanos(nanos as u64))
    );
}

#[test]
fn a_contemporary_instant_keeps_its_last_nanosecond() {
    // 2026-01-01T00:00:00.000000001Z
    let nanos = 1_767_225_600_000_000_001;
    let rayforce = from_unix_nanos(nanos).unwrap();
    assert_eq!(rayforce, 820_540_800_000_000_001);
    assert_eq!(to_unix_nanos(rayforce), Ok(nanos));
}

#[test]
fn a_negative_timespan_is_not_a_duration() {
    assert!(timespan_to_duration(-1).is_err());
    assert_eq!(
        timespan_to_duration(i64::MIN).unwrap_err().to_string(),
        "null timestamp"
    );
}

#[test]
fn a_timestamp_column_builds_the_same_value_as_the_vector() {
    let nanos = vec![0_i64, 843_048_000_500_000_000];
    let mut column = rayforce_ipc::Column::timestamp(2);
    for nanos in &nanos {
        column.add_i64(*nanos);
    }
    assert_eq!(round_trip(&Value::Col(column)), Value::Timestamps(nanos));
}

#[test]
fn symbols_and_strings_keep_their_identity_through_a_table() {
    let table = Value::table([
        (
            "instrument",
            Value::Syms(vec![Sym::intern("BTCUSDT"), Sym::intern("ETHUSDT")]),
        ),
        (
            "venue",
            Value::Strs(vec!["binance".into(), "coinbase".into()]),
        ),
    ])
    .unwrap();
    let decoded = round_trip(&table);
    assert_eq!(
        decoded.column("instrument").unwrap(),
        Some(&Value::Syms(vec![
            Sym::intern("BTCUSDT"),
            Sym::intern("ETHUSDT")
        ]))
    );
    assert_eq!(
        decoded.column("venue").unwrap(),
        Some(&Value::Strs(vec!["binance".into(), "coinbase".into()]))
    );
}
