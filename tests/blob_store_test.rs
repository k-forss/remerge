mod common;

use sha2::Digest;

#[tokio::test]
async fn store_blob_deduplicates_by_content_hash() {
    let state_dir = tempfile::TempDir::new().unwrap();

    let first = remerge_server::blob_store::store_blob(state_dir.path(), b"same-bytes")
        .await
        .expect("store first blob");
    let second = remerge_server::blob_store::store_blob(state_dir.path(), b"same-bytes")
        .await
        .expect("store second blob");
    let third = remerge_server::blob_store::store_blob(state_dir.path(), b"different-bytes")
        .await
        .expect("store third blob");

    assert_eq!(first, second);
    assert_ne!(first, third);
    assert_eq!(tokio::fs::read(&first.path).await.unwrap(), b"same-bytes");
    assert_eq!(
        tokio::fs::read(&third.path).await.unwrap(),
        b"different-bytes"
    );
}

#[tokio::test]
async fn store_blob_writes_metadata_for_canonical_raw_digest() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let bytes = b"metadata-bytes";
    let digest = hex::encode(sha2::Sha256::digest(bytes));

    let stored = remerge_server::blob_store::store_blob(state_dir.path(), bytes)
        .await
        .expect("store canonical blob");
    let metadata = remerge_server::blob_store::load_blob_metadata(state_dir.path(), &digest)
        .await
        .expect("load blob metadata");

    assert_eq!(stored.digest, digest);
    assert_eq!(metadata.canonical_digest, digest);
    assert_eq!(metadata.raw_size_bytes, bytes.len() as u64);
    assert!(metadata.encoded_variants.is_empty());
}

#[tokio::test]
async fn store_encoded_blob_variant_updates_blob_metadata() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let raw_bytes = b"canonical-raw";
    let digest = hex::encode(sha2::Sha256::digest(raw_bytes));

    remerge_server::blob_store::store_blob(state_dir.path(), raw_bytes)
        .await
        .expect("store canonical blob");
    let encoded = remerge_server::blob_store::store_encoded_blob_variant(
        state_dir.path(),
        &digest,
        raw_bytes.len() as u64,
        remerge_server::blob_store::BlobEncoding::Zstd,
        b"fake-zstd-payload",
    )
    .await
    .expect("store encoded blob variant");
    let metadata = remerge_server::blob_store::load_blob_metadata(state_dir.path(), &digest)
        .await
        .expect("load blob metadata");

    assert_eq!(
        tokio::fs::read(&encoded.path).await.unwrap(),
        b"fake-zstd-payload"
    );
    assert_eq!(
        encoded.path,
        remerge_server::blob_store::encoded_blob_path(
            state_dir.path(),
            &digest,
            remerge_server::blob_store::BlobEncoding::Zstd,
        )
        .unwrap()
    );
    assert_eq!(
        metadata
            .encoded_variants
            .get(&remerge_server::blob_store::BlobEncoding::Zstd),
        Some(&(b"fake-zstd-payload".len() as u64))
    );
    assert_eq!(metadata.raw_size_bytes, raw_bytes.len() as u64);
}

#[tokio::test]
async fn store_blob_creates_zstd_variant_for_worthwhile_payloads() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let raw_bytes = vec![b'a'; 256 * 1024];
    let digest = hex::encode(sha2::Sha256::digest(&raw_bytes));

    remerge_server::blob_store::store_blob(state_dir.path(), &raw_bytes)
        .await
        .expect("store compressible blob");
    let metadata = remerge_server::blob_store::load_blob_metadata(state_dir.path(), &digest)
        .await
        .expect("load blob metadata");
    let encoded_path = remerge_server::blob_store::encoded_blob_path(
        state_dir.path(),
        &digest,
        remerge_server::blob_store::BlobEncoding::Zstd,
    )
    .expect("encoded blob path");

    assert!(encoded_path.is_file());
    assert!(
        metadata
            .encoded_variants
            .contains_key(&remerge_server::blob_store::BlobEncoding::Zstd)
    );
    assert!(
        metadata.encoded_variants[&remerge_server::blob_store::BlobEncoding::Zstd]
            < raw_bytes.len() as u64
    );
}

#[tokio::test]
async fn store_blob_skips_zstd_variant_for_incompressible_payloads() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let mut seed = 0x1234_5678_9abc_def0u64;
    let raw_bytes: Vec<u8> = (0..256 * 1024)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed & 0xff) as u8
        })
        .collect();
    let digest = hex::encode(sha2::Sha256::digest(&raw_bytes));

    remerge_server::blob_store::store_blob(state_dir.path(), &raw_bytes)
        .await
        .expect("store incompressible blob");
    let metadata = remerge_server::blob_store::load_blob_metadata(state_dir.path(), &digest)
        .await
        .expect("load blob metadata");
    let encoded_path = remerge_server::blob_store::encoded_blob_path(
        state_dir.path(),
        &digest,
        remerge_server::blob_store::BlobEncoding::Zstd,
    )
    .expect("encoded blob path");

    assert!(!encoded_path.exists());
    assert!(metadata.encoded_variants.is_empty());
}

#[tokio::test]
async fn store_blob_streaming_copy_fallback_handles_large_payloads() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let raw_bytes = vec![b'a'; 512 * 1024];

    let stored = remerge_server::blob_store::store_blob(state_dir.path(), &raw_bytes)
        .await
        .expect("store canonical blob");

    assert_eq!(tokio::fs::read(&stored.path).await.unwrap(), raw_bytes);
}
