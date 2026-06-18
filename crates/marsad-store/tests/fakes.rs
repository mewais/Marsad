//! fakes.rs — exercise the Backend trait contract via MemoryBackend directly.
//!
//! Covers:
//! - Backend::read: key present → Some(bytes); key absent → None
//! - Backend::write: initial write stores bytes; overwrite replaces value
//! - Backend::list: prefix matching; empty store; single key; multiple keys with same prefix
//! - Backend::delete: existing key → Ok, subsequent read → None
//! - Backend::delete: missing key → Ok (delete-missing is a no-op per contract)
//! - MemoryBackend::new() creates an empty backend
//! - MemoryBackend::corrupt: corrupts a key so a subsequent read returns Corrupt error
//! - MemoryBackend implements Backend trait (type-level assertion)
//! - Overwrite preserves only the latest value (no phantom old data)
//! - Concurrent-style multiple reads return the same value

use marsad_store::{Backend, MemoryBackend, StoreError};

// ---------------------------------------------------------------------------
// MemoryBackend::new() — starts empty
// ---------------------------------------------------------------------------

#[test]
fn new_backend_is_empty() {
    let b = MemoryBackend::new();
    let keys = b.list("").expect("list on empty backend");
    assert!(keys.is_empty(), "new MemoryBackend must have no keys");
}

#[test]
fn new_backend_read_returns_none_for_any_key() {
    let b = MemoryBackend::new();
    let val = b.read("any-key").expect("read on empty backend must not error");
    assert!(val.is_none(), "read of missing key must return None");
}

// ---------------------------------------------------------------------------
// Backend::write + read
// ---------------------------------------------------------------------------

#[test]
fn write_then_read_returns_same_bytes() {
    let b = MemoryBackend::new();
    b.write("k1", b"hello world").expect("write");
    let val = b.read("k1").expect("read").expect("must be Some");
    assert_eq!(val, b"hello world");
}

#[test]
fn write_empty_bytes_then_read_returns_empty_vec() {
    let b = MemoryBackend::new();
    b.write("k-empty", b"").expect("write empty bytes");
    let val = b.read("k-empty").expect("read").expect("must be Some");
    assert!(val.is_empty(), "empty write must read back as empty vec");
}

#[test]
fn write_binary_bytes_survives_read() {
    let b = MemoryBackend::new();
    let bytes: Vec<u8> = (0u8..=255).collect();
    b.write("k-bin", &bytes).expect("write binary");
    let val = b.read("k-bin").expect("read").expect("must be Some");
    assert_eq!(val, bytes);
}

#[test]
fn read_absent_key_returns_none() {
    let b = MemoryBackend::new();
    b.write("present", b"data").expect("write");
    let val = b.read("absent").expect("read absent must not error");
    assert!(val.is_none(), "missing key must return None");
}

// ---------------------------------------------------------------------------
// Backend::write overwrite — only latest value survives
// ---------------------------------------------------------------------------

#[test]
fn overwrite_existing_key_replaces_value() {
    let b = MemoryBackend::new();
    b.write("k-ow", b"first").expect("first write");
    b.write("k-ow", b"second").expect("second write");
    let val = b.read("k-ow").expect("read").expect("must be Some");
    assert_eq!(val, b"second", "overwrite must replace value");
}

#[test]
fn overwrite_three_times_only_last_survives() {
    let b = MemoryBackend::new();
    for i in 0..3u8 {
        b.write("k-3x", &[i]).expect("write");
    }
    let val = b.read("k-3x").expect("read").expect("must be Some");
    assert_eq!(val, [2u8], "only third (last) write must survive");
}

// ---------------------------------------------------------------------------
// Backend::list — prefix matching
// ---------------------------------------------------------------------------

#[test]
fn list_prefix_returns_only_keys_with_that_prefix() {
    let b = MemoryBackend::new();
    b.write("foo:a", b"1").expect("write");
    b.write("foo:b", b"2").expect("write");
    b.write("bar:x", b"3").expect("write");
    b.write("baz:y", b"4").expect("write");

    let keys = b.list("foo:").expect("list foo:");
    assert_eq!(keys.len(), 2, "must return exactly 2 keys under foo:");
    assert!(keys.contains(&"foo:a".to_string()));
    assert!(keys.contains(&"foo:b".to_string()));
    assert!(!keys.contains(&"bar:x".to_string()));
    assert!(!keys.contains(&"baz:y".to_string()));
}

#[test]
fn list_empty_prefix_returns_all_keys() {
    let b = MemoryBackend::new();
    b.write("aaa", b"1").expect("write");
    b.write("bbb", b"2").expect("write");
    b.write("ccc", b"3").expect("write");

    let keys = b.list("").expect("list all");
    assert!(keys.contains(&"aaa".to_string()));
    assert!(keys.contains(&"bbb".to_string()));
    assert!(keys.contains(&"ccc".to_string()));
    assert_eq!(keys.len(), 3);
}

#[test]
fn list_no_matching_prefix_returns_empty() {
    let b = MemoryBackend::new();
    b.write("session:a", b"1").expect("write");
    let keys = b.list("workspace:").expect("list workspace:");
    assert!(
        keys.is_empty(),
        "list with no-match prefix must return empty vec, got: {keys:?}"
    );
}

#[test]
fn list_single_key_exact_prefix() {
    let b = MemoryBackend::new();
    b.write("only:one", b"v").expect("write");
    let keys = b.list("only:").expect("list only:");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0], "only:one");
}

#[test]
fn list_on_empty_backend_returns_empty() {
    let b = MemoryBackend::new();
    let keys = b.list("prefix:").expect("list on empty");
    assert!(keys.is_empty());
}

// ---------------------------------------------------------------------------
// Backend::delete
// ---------------------------------------------------------------------------

#[test]
fn delete_existing_key_returns_ok() {
    let b = MemoryBackend::new();
    b.write("del-me", b"data").expect("write");
    b.delete("del-me").expect("delete must return Ok");
}

#[test]
fn delete_existing_key_then_read_returns_none() {
    let b = MemoryBackend::new();
    b.write("del-read", b"payload").expect("write");
    b.delete("del-read").expect("delete");
    let val = b.read("del-read").expect("read after delete must not error");
    assert!(
        val.is_none(),
        "read after delete must return None, got: {val:?}"
    );
}

#[test]
fn delete_existing_key_removes_from_list() {
    let b = MemoryBackend::new();
    b.write("pfx:keep", b"1").expect("write keep");
    b.write("pfx:gone", b"2").expect("write gone");
    b.delete("pfx:gone").expect("delete");
    let keys = b.list("pfx:").expect("list");
    assert!(keys.contains(&"pfx:keep".to_string()), "pfx:keep must remain");
    assert!(
        !keys.contains(&"pfx:gone".to_string()),
        "pfx:gone must be absent after delete"
    );
}

#[test]
fn delete_missing_key_returns_ok() {
    // The spec says delete of a missing key is a no-op (idempotent). Must not error.
    let b = MemoryBackend::new();
    let result = b.delete("this-key-does-not-exist");
    assert!(
        result.is_ok(),
        "delete of missing key must return Ok, got: {result:?}"
    );
}

#[test]
fn delete_missing_key_twice_both_ok() {
    let b = MemoryBackend::new();
    b.delete("ghost").expect("first delete of missing must be Ok");
    b.delete("ghost").expect("second delete of missing must be Ok");
}

// ---------------------------------------------------------------------------
// MemoryBackend::corrupt — key becomes unreadable (StoreError::Corrupt)
// ---------------------------------------------------------------------------

#[test]
fn corrupt_then_read_returns_corrupt_error() {
    let b = MemoryBackend::new();
    b.write("corrupt-me", b"some data").expect("write");
    b.corrupt("corrupt-me");
    let result = b.read("corrupt-me");
    match result {
        Err(StoreError::Corrupt { key }) => {
            assert!(
                key.contains("corrupt-me"),
                "Corrupt key must reference the corrupted key, got: {key}"
            );
        }
        other => panic!("expected Corrupt error after corrupt(), got: {other:?}"),
    }
}

#[test]
fn corrupt_does_not_affect_other_keys() {
    let b = MemoryBackend::new();
    b.write("corrupt-target", b"bad").expect("write target");
    b.write("intact-key", b"good data").expect("write intact");
    b.corrupt("corrupt-target");

    // The non-corrupted key must still read fine.
    let val = b.read("intact-key").expect("read intact key").expect("Some");
    assert_eq!(val, b"good data", "intact key must not be affected by corrupt()");
}

#[test]
fn corrupt_absent_key_does_not_error_on_corrupt_call() {
    // corrupt() on a key that was never written — implementation may or may not insert
    // a sentinel, but it must not panic.
    let b = MemoryBackend::new();
    b.corrupt("never-written"); // must not panic
}

// ---------------------------------------------------------------------------
// Multiple reads return consistent value
// ---------------------------------------------------------------------------

#[test]
fn multiple_reads_return_same_value() {
    let b = MemoryBackend::new();
    b.write("stable", b"constant").expect("write");
    for _ in 0..10 {
        let val = b.read("stable").expect("read").expect("Some");
        assert_eq!(val, b"constant");
    }
}

// ---------------------------------------------------------------------------
// Type-level: MemoryBackend is a Backend (compile-time check)
// ---------------------------------------------------------------------------

#[test]
fn memory_backend_satisfies_backend_trait() {
    fn accepts_backend(_b: &dyn Backend) {}
    let b = MemoryBackend::new();
    accepts_backend(&b);
}

// ---------------------------------------------------------------------------
// Multiple independent MemoryBackend instances don't share state
// ---------------------------------------------------------------------------

#[test]
fn two_memory_backends_are_independent() {
    let b1 = MemoryBackend::new();
    let b2 = MemoryBackend::new();
    b1.write("shared-key", b"value-in-b1").expect("write b1");
    let val = b2.read("shared-key").expect("read from b2");
    assert!(
        val.is_none(),
        "b2 must not see data written to b1"
    );
}

// ---------------------------------------------------------------------------
// Key names with special characters and long keys
// ---------------------------------------------------------------------------

#[test]
fn key_with_colon_separators_works() {
    let b = MemoryBackend::new();
    b.write("session:host:project:branch:id", b"complex").expect("write");
    let keys = b.list("session:host:").expect("list complex prefix");
    assert!(keys.contains(&"session:host:project:branch:id".to_string()));
}

#[test]
fn write_and_read_large_value() {
    let b = MemoryBackend::new();
    let large = vec![42u8; 100_000]; // 100 KB
    b.write("big-key", &large).expect("write large value");
    let val = b.read("big-key").expect("read large").expect("Some");
    assert_eq!(val.len(), 100_000);
    assert!(val.iter().all(|&byte| byte == 42));
}
