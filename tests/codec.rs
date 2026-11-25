//! Golden bytes: what the format says an atom or a vector looks like.
//!
//! The round-trip tests in `src/value.rs` prove this client agrees with itself. These pin the
//! layout to the numbers in `include/rayforce.h`, and to the byte sequences the OCaml client's
//! suite asserts, so a decoder that reads `i32` where the format says `i64` fails here.

use rayforce_ipc::tag;
use rayforce_ipc::{Sym, Value};

fn vector_head(tag: u8, len: i64) -> Vec<u8> {
    let mut out = vec![tag, 0];
    out.extend_from_slice(&len.to_le_bytes());
    out
}

fn atom(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(tag as i8).wrapping_neg() as u8, 0];
    out.extend_from_slice(payload);
    out
}

fn encoded(value: &Value) -> Vec<u8> {
    let mut buf = Vec::new();
    value.fill(&mut buf).expect("encode");
    assert_eq!(
        buf.len(),
        value.size(),
        "size disagrees with fill for {value:?}"
    );
    buf
}

#[test]
fn null_is_one_byte() {
    assert_eq!(encoded(&Value::Null), [0x7e]);
}

#[test]
fn atoms_use_the_negated_tag_and_a_flags_byte() {
    assert_eq!(encoded(&Value::Bool(true)), [0xff, 0x00, 0x01]);
    assert_eq!(encoded(&Value::Bool(false)), [0xff, 0x00, 0x00]);
    // 200 is past i8::MAX: a signed-byte read would turn it into -56.
    assert_eq!(encoded(&Value::U8(200)), [0xfe, 0x00, 0xc8]);
    assert_eq!(encoded(&Value::I16(-3)), [0xfd, 0x00, 0xfd, 0xff]);
    assert_eq!(
        encoded(&Value::I32(70_000)),
        atom(tag::I32, &[0x70, 0x11, 0x01, 0x00])
    );
    // Days since 2000-01-01, four bytes, not eight.
    assert_eq!(
        encoded(&Value::Date(9_000)),
        atom(tag::DATE, &[0x28, 0x23, 0x00, 0x00])
    );
    assert_eq!(
        encoded(&Value::Time(1_234)),
        atom(tag::TIME, &[0xd2, 0x04, 0x00, 0x00])
    );
    // Past u32::MAX: a 32-bit truncation would lose the top byte.
    assert_eq!(
        encoded(&Value::I64(4_294_967_304)),
        atom(tag::I64, &[0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00])
    );
    assert_eq!(
        encoded(&Value::F64(1.5)),
        atom(tag::F64, &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8, 0x3f])
    );
    assert_eq!(
        encoded(&Value::Timestamp(123)),
        atom(
            tag::TIMESTAMP,
            &[0x7b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        )
    );
    // Tag 16, not 14: the timespan is not contiguous with string and symbol.
    assert_eq!(
        encoded(&Value::Timespan(1)),
        atom(
            tag::TIMESPAN,
            &[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        )
    );
    assert_eq!(
        encoded(&Value::Sym(Sym::intern("upd"))),
        atom(tag::SYM, b"upd\0")
    );
    let mut string = atom(tag::STR, &[0x07, 0, 0, 0, 0, 0, 0, 0]);
    string.extend_from_slice(b"(+ 1 2)");
    assert_eq!(encoded(&Value::Str("(+ 1 2)".into())), string);
    assert_eq!(
        encoded(&Value::F32(0x7fc0_1234)),
        atom(tag::F32, &[0x34, 0x12, 0xc0, 0x7f])
    );
    assert_eq!(
        encoded(&Value::Guid(*b"0123456789abcdef")),
        atom(tag::GUID, b"0123456789abcdef")
    );
}

#[test]
fn an_error_is_a_nul_padded_code_and_a_length_prefixed_message() {
    assert_eq!(
        encoded(&Value::Error {
            code: "domain".into(),
            message: String::new()
        }),
        [0x7f, b'd', b'o', b'm', b'a', b'i', b'n', 0, 0, 0]
    );
    let mut expected = vec![0x7f];
    expected.extend_from_slice(b"type\0\0\0\0");
    expected.push(21);
    expected.extend_from_slice(b"expected i64, got sym");
    assert_eq!(
        encoded(&Value::Error {
            code: "type".into(),
            message: "expected i64, got sym".into()
        }),
        expected
    );
}

#[test]
fn atom_flags_replace_the_zero_byte() {
    assert_eq!(
        encoded(&Value::AtomFlags(1, Box::new(Value::Bool(false)))),
        [0xff, 0x01, 0x00]
    );
    assert_eq!(
        encoded(&Value::AtomFlags(
            32,
            Box::new(Value::Sym(Sym::intern("hello")))
        )),
        atom_with_flags(tag::SYM, 32, b"hello\0")
    );
}

fn atom_with_flags(tag: u8, flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(tag as i8).wrapping_neg() as u8, flags];
    out.extend_from_slice(payload);
    out
}

#[test]
fn vectors_carry_a_type_an_attrs_byte_a_length_and_raw_cells() {
    let mut bools = vector_head(tag::BOOL, 3);
    bools.extend_from_slice(&[1, 0, 1]);
    assert_eq!(encoded(&Value::Bools(vec![true, false, true])), bools);

    let mut shorts = vector_head(tag::I16, 2);
    shorts.extend_from_slice(&i16::MIN.to_le_bytes());
    shorts.extend_from_slice(&i16::MAX.to_le_bytes());
    assert_eq!(encoded(&Value::I16s(vec![i16::MIN, i16::MAX])), shorts);

    let mut int32s = vector_head(tag::I32, 2);
    int32s.extend_from_slice(&i32::MIN.to_le_bytes());
    int32s.extend_from_slice(&i32::MAX.to_le_bytes());
    assert_eq!(encoded(&Value::I32s(vec![i32::MIN, i32::MAX])), int32s);

    let mut dates = vector_head(tag::DATE, 2);
    dates.extend_from_slice(&0_i32.to_le_bytes());
    dates.extend_from_slice(&(-1_i32).to_le_bytes());
    assert_eq!(encoded(&Value::Dates(vec![0, -1])), dates);

    let mut times = vector_head(tag::TIME, 2);
    times.extend_from_slice(&0_i32.to_le_bytes());
    times.extend_from_slice(&1_234_i32.to_le_bytes());
    assert_eq!(encoded(&Value::Times(vec![0, 1_234])), times);

    let mut float32s = vector_head(tag::F32, 3);
    for bits in [0_u32, 0x7fc0_1234, i32::MIN as u32] {
        float32s.extend_from_slice(&bits.to_le_bytes());
    }
    assert_eq!(
        encoded(&Value::F32s(vec![0, 0x7fc0_1234, i32::MIN as u32])),
        float32s
    );

    let mut guids = vector_head(tag::GUID, 2);
    guids.extend_from_slice(b"0123456789abcdef");
    guids.extend_from_slice(b"fedcba9876543210");
    assert_eq!(
        encoded(&Value::Guids(vec![
            *b"0123456789abcdef",
            *b"fedcba9876543210"
        ])),
        guids
    );

    let mut ints = vector_head(tag::I64, 3);
    for value in [0_i64, -1, i64::MAX] {
        ints.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(encoded(&Value::I64s(vec![0, -1, i64::MAX])), ints);

    let mut floats = vector_head(tag::F64, 3);
    for value in [0.0_f64, -1.25, f64::MAX] {
        floats.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(encoded(&Value::F64s(vec![0.0, -1.25, f64::MAX])), floats);

    let mut timestamps = vector_head(tag::TIMESTAMP, 2);
    timestamps.extend_from_slice(&0_i64.to_le_bytes());
    timestamps.extend_from_slice(&1_i64.to_le_bytes());
    assert_eq!(encoded(&Value::Timestamps(vec![0, 1])), timestamps);

    let mut syms = vector_head(tag::SYM, 3);
    syms.extend_from_slice(b"BTCUSDT\0ETHUSDT\0BTCUSDT\0");
    assert_eq!(
        encoded(&Value::Syms(vec![
            Sym::intern("BTCUSDT"),
            Sym::intern("ETHUSDT"),
            Sym::intern("BTCUSDT"),
        ])),
        syms
    );

    let mut strings = vector_head(tag::STR, 3);
    strings.extend_from_slice(&0_i64.to_le_bytes());
    strings.extend_from_slice(&1_i64.to_le_bytes());
    strings.extend_from_slice(b"a");
    strings.extend_from_slice(&19_i64.to_le_bytes());
    strings.extend_from_slice(b"the quick brown fox");
    assert_eq!(
        encoded(&Value::Strs(vec![
            "".into(),
            "a".into(),
            "the quick brown fox".into()
        ])),
        strings
    );
}

#[test]
fn empty_vectors_are_a_head_with_a_zero_length() {
    assert_eq!(encoded(&Value::Syms(Vec::new())), vector_head(tag::SYM, 0));
    assert_eq!(encoded(&Value::I64s(Vec::new())), vector_head(tag::I64, 0));
    assert_eq!(
        encoded(&Value::Bools(Vec::new())),
        vector_head(tag::BOOL, 0)
    );
    assert_eq!(encoded(&Value::Strs(Vec::new())), vector_head(tag::STR, 0));
}

#[test]
fn named_builtins_are_a_tag_and_a_nul_terminated_name_with_no_attrs() {
    let mut unary = vec![tag::UNARY];
    unary.extend_from_slice(b"long_builtin_name\0");
    assert_eq!(encoded(&Value::Unary("long_builtin_name".into())), unary);
    assert_eq!(encoded(&Value::Binary("+".into())), [tag::BINARY, b'+', 0]);
    assert_eq!(
        encoded(&Value::Variadic("list".into())),
        b"\x67list\0".to_vec()
    );
}

#[test]
fn a_table_is_a_schema_vector_then_a_list_of_columns() {
    let table = Value::table([
        ("px", Value::I64s(vec![1, 2])),
        ("live", Value::Bools(vec![true, false])),
    ])
    .unwrap();
    let mut expected = vec![tag::TABLE, 0];
    // The schema vector carries attrs 3, RAY_SYM_W64, as the C serializer writes it.
    let mut schema = vec![tag::SYM, 3];
    schema.extend_from_slice(&2_i64.to_le_bytes());
    schema.extend_from_slice(b"px\0live\0");
    expected.extend_from_slice(&schema);
    expected.extend_from_slice(&vector_head(tag::LIST, 2));
    let mut px = vector_head(tag::I64, 2);
    px.extend_from_slice(&1_i64.to_le_bytes());
    px.extend_from_slice(&2_i64.to_le_bytes());
    expected.extend_from_slice(&px);
    let mut live = vector_head(tag::BOOL, 2);
    live.extend_from_slice(&[1, 0]);
    expected.extend_from_slice(&live);
    assert_eq!(encoded(&table), expected);
}

#[test]
fn a_dict_is_keys_then_values() {
    let dict = Value::Dict(
        Box::new(Value::Syms(vec![Sym::intern("a"), Sym::intern("b")])),
        Box::new(Value::I64s(vec![1, 2])),
    );
    let mut expected = vec![tag::DICT, 0];
    let mut keys = vector_head(tag::SYM, 2);
    keys.extend_from_slice(b"a\0b\0");
    expected.extend_from_slice(&keys);
    let mut values = vector_head(tag::I64, 2);
    values.extend_from_slice(&1_i64.to_le_bytes());
    values.extend_from_slice(&2_i64.to_le_bytes());
    expected.extend_from_slice(&values);
    assert_eq!(encoded(&dict), expected);
}

#[test]
fn a_lambda_records_whether_it_closed_over_anything() {
    let without = Value::Lambda {
        params: Box::new(Value::Syms(Vec::new())),
        body: Box::new(Value::I64(42)),
        closure: None,
    };
    let mut expected = vec![tag::LAMBDA, 0];
    expected.extend_from_slice(&vector_head(tag::SYM, 0));
    expected.extend_from_slice(&atom(tag::I64, &42_i64.to_le_bytes()));
    assert_eq!(encoded(&without), expected);

    let with = Value::Lambda {
        params: Box::new(Value::Syms(Vec::new())),
        body: Box::new(Value::I64(42)),
        closure: Some(Box::new(Value::Dict(
            Box::new(Value::Syms(Vec::new())),
            Box::new(Value::List(Vec::new())),
        ))),
    };
    let mut expected = vec![tag::LAMBDA, 1];
    expected.extend_from_slice(&vector_head(tag::SYM, 0));
    expected.extend_from_slice(&atom(tag::I64, &42_i64.to_le_bytes()));
    expected.extend_from_slice(&[tag::DICT, 0]);
    expected.extend_from_slice(&vector_head(tag::SYM, 0));
    expected.extend_from_slice(&vector_head(tag::LIST, 0));
    assert_eq!(encoded(&with), expected);
}

#[test]
fn a_nested_message_lays_out_back_to_back() {
    let message = Value::List(vec![
        Value::Sym(Sym::intern("upd")),
        Value::List(vec![Value::I64(1)]),
        Value::Null,
    ]);
    let mut expected = vector_head(tag::LIST, 3);
    expected.extend_from_slice(&atom(tag::SYM, b"upd\0"));
    expected.extend_from_slice(&vector_head(tag::LIST, 1));
    expected.extend_from_slice(&atom(tag::I64, &1_i64.to_le_bytes()));
    expected.push(tag::NULL);
    assert_eq!(encoded(&message), expected);
}
