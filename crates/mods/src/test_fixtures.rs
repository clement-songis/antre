//! Hand-written component fixtures (WAT) shared by the host and service
//! tests, so they run without a wasm32 toolchain. They implement the
//! canonical ABI by hand: `tools.call` returns a pointer to
//! `{tag: u8, ptr: u32, len: u32}`, `hooks.handle` to
//! `{tag: u8, (opt_tag: u8, ptr, len) | (ptr, len)}`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use {crate::host::HostLimits, std::time::Duration};

pub(crate) const OK_JSON_AT: u32 = 48; // "{\"ok\":true}" (11 bytes)
pub(crate) const STORED_AT: u32 = 80; // "\"stored\"" (8 bytes)
pub(crate) const REASON_AT: u32 = 96; // "nope" (4 bytes)
pub(crate) const MODIFIED_AT: u32 = 112; // "{\"x\":1}" (7 bytes)

/// Build a plugin component. `call` and `handle` are core function bodies
/// with signature `(param i32 i32 i32 i32) (result i32)`.
pub(crate) fn plugin(call: &str, handle: &str) -> Vec<u8> {
    let wat = format!(
        r#"(component
  (import "antre:mods/host@0.1.0" (instance $host
    (export "kv-get" (func (param "key" string) (result (result (option string) (error string)))))
    (export "kv-set" (func (param "key" string) (param "value" string) (result (result (error string)))))
  ))
  (core module $Mem
    (memory (export "memory") 1)
    (global $bump (mut i32) (i32.const 8192))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $ret i32)
      (global.set $bump
        (i32.and
          (i32.add (global.get $bump) (i32.sub (local.get 2) (i32.const 1)))
          (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
      (local.set $ret (global.get $bump))
      (global.set $bump (i32.add (global.get $bump) (local.get 3)))
      (local.get $ret)))
  (core instance $mem (instantiate $Mem))
  (core func $kv_set (canon lower (func $host "kv-set") (memory $mem "memory") (realloc (func $mem "realloc"))))
  (core func $kv_get (canon lower (func $host "kv-get") (memory $mem "memory") (realloc (func $mem "realloc"))))
  (core module $Main
    (import "mem" "memory" (memory 1))
    (import "host" "kv-set" (func $kv_set (param i32 i32 i32 i32 i32)))
    (import "host" "kv-get" (func $kv_get (param i32 i32 i32)))
    (data (i32.const 16) "key")
    (data (i32.const 32) "value")
    (data (i32.const {OK_JSON_AT}) "{{\"ok\":true}}")
    (data (i32.const {STORED_AT}) "\"stored\"")
    (data (i32.const {REASON_AT}) "nope")
    (data (i32.const {MODIFIED_AT}) "{{\"x\":1}}")
    (func (export "call") (param i32 i32 i32 i32) (result i32) {call})
    (func (export "handle") (param i32 i32 i32 i32) (result i32) {handle}))
  (core instance $main (instantiate $Main
    (with "mem" (instance $mem))
    (with "host" (instance (export "kv-set" (func $kv_set)) (export "kv-get" (func $kv_get))))))
  (func $call (param "name" string) (param "params-json" string) (result (result string (error string)))
    (canon lift (core func $main "call") (memory $mem "memory") (realloc (func $mem "realloc"))))
  (func $handle (param "event" string) (param "payload-json" string) (result (result (option string) (error string)))
    (canon lift (core func $main "handle") (memory $mem "memory") (realloc (func $mem "realloc"))))
  (instance $tools (export "call" (func $call)))
  (instance $hooks (export "handle" (func $handle)))
  (export "antre:mods/tools@0.1.0" (instance $tools))
  (export "antre:mods/hooks@0.1.0" (instance $hooks))
)"#
    );
    wat::parse_str(&wat).expect("fixture WAT must parse")
}

pub(crate) fn ok_string(ptr: &str, len: &str) -> String {
    format!(
        "(i32.store8 (i32.const 256) (i32.const 0)) (i32.store (i32.const 260) {ptr}) (i32.store (i32.const 264) {len}) (i32.const 256)"
    )
}

pub(crate) fn err_string(ptr: &str, len: &str) -> String {
    format!(
        "(i32.store8 (i32.const 256) (i32.const 1)) (i32.store (i32.const 260) {ptr}) (i32.store (i32.const 264) {len}) (i32.const 256)"
    )
}

pub(crate) fn call_ok_json() -> String {
    ok_string(&format!("(i32.const {OK_JSON_AT})"), "(i32.const 11)")
}

/// Echo the params string back.
pub(crate) fn call_echo() -> String {
    ok_string("(local.get 2)", "(local.get 3)")
}

pub(crate) fn call_spin() -> String {
    "(loop $l (br $l)) (unreachable)".to_string()
}

pub(crate) fn call_grow(pages: u32) -> String {
    format!(
        "(if (i32.eq (memory.grow (i32.const {pages})) (i32.const -1)) (then (unreachable))) {}",
        call_ok_json()
    )
}

pub(crate) fn call_kv_set() -> String {
    format!(
        "(call $kv_set (i32.const 16) (i32.const 3) (i32.const 32) (i32.const 5) (i32.const 512))
         (if (result i32) (i32.eqz (i32.load8_u (i32.const 512)))
           (then {})
           (else {}))",
        ok_string(&format!("(i32.const {STORED_AT})"), "(i32.const 8)"),
        err_string("(i32.load (i32.const 516))", "(i32.load (i32.const 520))"),
    )
}

/// Read back `key` and return its value (or `null` JSON when absent).
pub(crate) fn call_kv_get() -> String {
    format!(
        "(call $kv_get (i32.const 16) (i32.const 3) (i32.const 512))
         (if (result i32) (i32.load8_u (i32.const 512))
           (then {})
           (else (if (result i32) (i32.load8_u (i32.const 516))
             (then {})
             (else {}))))",
        err_string("(i32.load (i32.const 516))", "(i32.load (i32.const 520))"),
        ok_string("(i32.load (i32.const 520))", "(i32.load (i32.const 524))"),
        ok_string(&format!("(i32.const {STORED_AT})"), "(i32.const 0)"),
    )
}

pub(crate) fn call_big_output(len: u32) -> String {
    ok_string("(i32.const 4096)", &format!("(i32.const {len})"))
}

pub(crate) fn call_tool_error() -> String {
    err_string(&format!("(i32.const {REASON_AT})"), "(i32.const 4)")
}

pub(crate) fn handle_continue() -> String {
    "(i32.store8 (i32.const 300) (i32.const 0)) (i32.store8 (i32.const 304) (i32.const 0)) (i32.const 300)".to_string()
}

pub(crate) fn handle_modify() -> String {
    format!(
        "(i32.store8 (i32.const 300) (i32.const 0)) (i32.store8 (i32.const 304) (i32.const 1)) (i32.store (i32.const 308) (i32.const {MODIFIED_AT})) (i32.store (i32.const 312) (i32.const 7)) (i32.const 300)"
    )
}

pub(crate) fn handle_block() -> String {
    format!(
        "(i32.store8 (i32.const 300) (i32.const 1)) (i32.store (i32.const 304) (i32.const {REASON_AT})) (i32.store (i32.const 308) (i32.const 4)) (i32.const 300)"
    )
}

pub(crate) fn test_limits() -> HostLimits {
    HostLimits {
        fuel: 5_000_000,
        memory_bytes: 2 * 1024 * 1024,
        timeout: Duration::from_millis(300),
        epoch_interval: Duration::from_millis(10),
        max_output_bytes: 1024,
        ..HostLimits::default()
    }
}
