#![no_main]

use libfuzzer_sys::fuzz_target;
use remerge::args::extract_package_atoms;
use remerge_types::validation::validate_atom;

fuzz_target!(|data: &[u8]| {
    let argv = String::from_utf8_lossy(data)
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    for atom in extract_package_atoms(&argv) {
        let _ = validate_atom(&atom);
    }
});