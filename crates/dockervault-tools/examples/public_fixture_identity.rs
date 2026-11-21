//! TEST ONLY: emit an original PUBLIC synthetic identity for isolated offline CI.
//! Never installed in runtime/recovery images or used to load operator keys.
#[path = "../../dockervault-core/tests/common/mod.rs"]
mod common;
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let compressed = match args.as_slice() {
        [kind] if kind == "gzip" => true,
        [kind] if kind == "tar" => false,
        _ => panic!("expected public fixture kind: tar or gzip"),
    };
    print!("{}", common::public_identity(compressed));
}
