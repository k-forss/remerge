use std::io::{self, Cursor};

pub const ZSTD_COMPRESSION_LEVEL: i32 = 3;
pub const MIN_ZSTD_VARIANT_BYTES: usize = 64 * 1024;
pub const MIN_ZSTD_SAVINGS_NUMERATOR: u64 = 10;
pub const MIN_ZSTD_SAVINGS_DENOMINATOR: u64 = 100;

pub fn compression_is_worthwhile(raw_size_bytes: u64, encoded_size_bytes: u64) -> bool {
    encoded_size_bytes < raw_size_bytes
        && raw_size_bytes.saturating_sub(encoded_size_bytes) * MIN_ZSTD_SAVINGS_DENOMINATOR
            >= raw_size_bytes * MIN_ZSTD_SAVINGS_NUMERATOR
}

pub fn encode_zstd_if_worthwhile(raw_bytes: &[u8]) -> io::Result<Option<Vec<u8>>> {
    if raw_bytes.len() < MIN_ZSTD_VARIANT_BYTES {
        return Ok(None);
    }

    let encoded = zstd::stream::encode_all(Cursor::new(raw_bytes), ZSTD_COMPRESSION_LEVEL)?;
    if compression_is_worthwhile(raw_bytes.len() as u64, encoded.len() as u64) {
        Ok(Some(encoded))
    } else {
        Ok(None)
    }
}

pub fn decode_zstd(encoded_bytes: &[u8]) -> io::Result<Vec<u8>> {
    zstd::stream::decode_all(Cursor::new(encoded_bytes))
}

#[cfg(test)]
mod tests {
    use super::{
        MIN_ZSTD_VARIANT_BYTES, compression_is_worthwhile, decode_zstd, encode_zstd_if_worthwhile,
    };

    #[test]
    fn skips_small_payloads() {
        let payload = vec![b'a'; MIN_ZSTD_VARIANT_BYTES - 1];

        let encoded = encode_zstd_if_worthwhile(&payload).expect("encode small payload");

        assert!(encoded.is_none());
    }

    #[test]
    fn compresses_and_roundtrips_worthwhile_payloads() {
        let payload = vec![b'a'; 256 * 1024];

        let encoded = encode_zstd_if_worthwhile(&payload)
            .expect("encode payload")
            .expect("worthwhile zstd variant");

        assert!(compression_is_worthwhile(
            payload.len() as u64,
            encoded.len() as u64
        ));
        assert_eq!(decode_zstd(&encoded).expect("decode payload"), payload);
    }
}
