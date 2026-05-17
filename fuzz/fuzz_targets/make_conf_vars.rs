#![no_main]

use libfuzzer_sys::fuzz_target;
use remerge::portage::PortageReader;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let vars = PortageReader::parse_make_conf_vars(input.as_ref());

    for (key, value) in vars {
        let _ = (key.len(), value.len());
    }
});