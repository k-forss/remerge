//! Phase 5 — Final-state parity integration tests.

mod common;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::Path as AxumPath;
use axum::extract::State;
use axum::http::{Response, StatusCode};
use axum::routing::get;
use remerge::args::{
    reconcile_fetched_distfiles_into, reconcile_final_state_parity_into,
    run_local_emerge_with_program,
};
use remerge::client::RemergeClient;
use remerge_types::portage::SnapshotEntry;
use remerge_types::workorder::{
    ParityDirectoryEntry, ParityFileEntry, ParityManifest, ParitySymlinkEntry, WorkorderResult,
};
use remerge_worker::parity::capture_final_state_parity_from;
use sha2::Digest;

async fn spawn_blob_server(
    blobs: HashMap<String, Vec<u8>>,
    requests: Arc<Mutex<Vec<String>>>,
) -> String {
    #[derive(Clone)]
    struct BlobState {
        blobs: Arc<HashMap<String, Vec<u8>>>,
        requests: Arc<Mutex<Vec<String>>>,
    }

    async fn serve_blob(
        AxumPath(digest): AxumPath<String>,
        State(state): State<BlobState>,
    ) -> Response<Body> {
        state.requests.lock().unwrap().push(digest.clone());

        match state.blobs.get(&digest) {
            Some(payload) => Response::builder()
                .status(StatusCode::OK)
                .header("content-length", payload.len().to_string())
                .body(Body::from(payload.clone()))
                .expect("blob response"),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("missing blob {digest}")))
                .expect("missing blob response"),
        }
    }

    let state = BlobState {
        blobs: Arc::new(blobs),
        requests,
    };

    let app = Router::new()
        .route("/api/v1/snapshots/blobs/{digest}", get(serve_blob))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind blob test server");
    let address = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve blob test app");
    });

    format!("http://{address}")
}

async fn spawn_zstd_blob_server(
    blobs: HashMap<String, Vec<u8>>,
    requests: Arc<Mutex<Vec<String>>>,
) -> String {
    #[derive(Clone)]
    struct BlobState {
        blobs: Arc<HashMap<String, Vec<u8>>>,
        requests: Arc<Mutex<Vec<String>>>,
    }

    async fn serve_blob(
        AxumPath(digest): AxumPath<String>,
        State(state): State<BlobState>,
    ) -> Response<Body> {
        state.requests.lock().unwrap().push(digest.clone());

        match state.blobs.get(&digest) {
            Some(payload) => {
                let encoded = zstd::stream::encode_all(std::io::Cursor::new(payload.clone()), 3)
                    .expect("encode zstd payload");
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-length", encoded.len().to_string())
                    .header("content-encoding", "zstd")
                    .body(Body::from(encoded))
                    .expect("blob response")
            }
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("missing blob {digest}")))
                .expect("missing blob response"),
        }
    }

    let state = BlobState {
        blobs: Arc::new(blobs),
        requests,
    };

    let app = Router::new()
        .route("/api/v1/snapshots/blobs/{digest}", get(serve_blob))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind blob test server");
    let address = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve blob test app");
    });

    format!("http://{address}")
}

fn write_file_with_mtime(path: &Path, contents: &[u8], mtime_secs: i64) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(mtime_secs, 0)).unwrap();
}

fn snapshot_regular_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                visit(root, &path, files);
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                files.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn write_fake_emerge_script(path: &Path) {
    let script = r#"#!/bin/sh
set -eu

: "${ROOT:?ROOT must be set}"
: "${PORTAGE_BINHOST:?PORTAGE_BINHOST must be set}"

world="$ROOT/var/lib/portage/world"
metadata="$ROOT/var/db/repos/gentoo/metadata/md5-cache/dev-libs-demo-1.0"
pkgdir="$ROOT/var/db/pkg/dev-libs/demo-1.0"

mkdir -p "$pkgdir"
cat "$world" "$metadata" > "$pkgdir/CONTENTS"
printf '%s\n' "$PORTAGE_BINHOST" > "$pkgdir/BINHOST"
printf '%s\n' "$*" > "$pkgdir/ARGS"
"#;

    std::fs::write(path, script).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn write_fake_distfile_emerge_script(path: &Path) {
    let script = r#"#!/bin/sh
set -eu

: "${ROOT:?ROOT must be set}"
: "${PORTAGE_BINHOST:?PORTAGE_BINHOST must be set}"

distfile="$ROOT/var/cache/distfiles/demo-1.0.tar.xz"
pkgdir="$ROOT/var/db/pkg/dev-libs/demo-1.0"

mkdir -p "$pkgdir"
cat "$distfile" > "$pkgdir/CONTENTS"
printf '%s\n' "$PORTAGE_BINHOST" > "$pkgdir/BINHOST"
printf '%s\n' "$*" > "$pkgdir/ARGS"
"#;

    std::fs::write(path, script).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[tokio::test]
async fn capture_final_state_parity_covers_the_approved_include_set() {
    let repo_root = tempfile::TempDir::new().unwrap();
    let eclass_root = tempfile::TempDir::new().unwrap();
    let binpkg_root = tempfile::TempDir::new().unwrap();
    let portage_root = tempfile::TempDir::new().unwrap();
    let output = tempfile::TempDir::new().unwrap();

    let repo_metadata = repo_root.path().join("gentoo/metadata/md5-cache");
    let repo_packages = repo_root.path().join("gentoo/Packages");
    let eclass_file = eclass_root.path().join("5-23/amd64.cache");
    let local_packages = binpkg_root.path().join("Packages");
    let world_file = portage_root.path().join("world");
    #[cfg(unix)]
    let world_link = portage_root.path().join("make.profile");

    tokio::fs::create_dir_all(&repo_metadata).await.unwrap();
    tokio::fs::create_dir_all(eclass_file.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        repo_root.path().join("gentoo/metadata/timestamp.chk"),
        b"1234\n",
    )
    .await
    .unwrap();
    tokio::fs::write(repo_metadata.join("dev-libs-demo-1.0"), b"EAPI=8\n")
        .await
        .unwrap();
    tokio::fs::write(&repo_packages, b"repo packages\n")
        .await
        .unwrap();
    tokio::fs::write(&eclass_file, b"cache entry\n")
        .await
        .unwrap();
    tokio::fs::write(&local_packages, b"PKGDIR index\n")
        .await
        .unwrap();
    tokio::fs::write(&world_file, b"app-misc/hello\n")
        .await
        .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "/var/db/repos/gentoo/profiles/default/linux/amd64/23.0",
        &world_link,
    )
    .unwrap();

    let manifest = capture_final_state_parity_from(
        output.path(),
        repo_root.path(),
        eclass_root.path(),
        &local_packages,
        portage_root.path(),
    )
    .await
    .expect("capture final-state parity");

    assert!(
        manifest
            .files
            .contains_key("var/db/repos/gentoo/metadata/timestamp.chk")
    );
    assert!(
        manifest
            .files
            .contains_key("var/db/repos/gentoo/metadata/md5-cache/dev-libs-demo-1.0")
    );
    assert!(manifest.files.contains_key("var/db/repos/gentoo/Packages"));
    assert!(
        manifest
            .files
            .contains_key("var/cache/eclass/5-23/amd64.cache")
    );
    assert!(manifest.files.contains_key("var/cache/binpkgs/Packages"));
    assert!(manifest.files.contains_key("var/lib/portage/world"));
    assert!(manifest.directories.contains_key("var/lib/portage"));
    assert!(
        manifest
            .directories
            .contains_key("var/db/repos/gentoo/metadata")
    );
    #[cfg(unix)]
    assert!(
        manifest
            .symlinks
            .contains_key("var/lib/portage/make.profile")
    );
}

#[tokio::test]
async fn capture_final_state_parity_excludes_client_owned_and_temporary_paths() {
    let repo_root = tempfile::TempDir::new().unwrap();
    let eclass_root = tempfile::TempDir::new().unwrap();
    let binpkg_root = tempfile::TempDir::new().unwrap();
    let portage_root = tempfile::TempDir::new().unwrap();
    let output = tempfile::TempDir::new().unwrap();
    let distfiles_root = tempfile::TempDir::new().unwrap();
    let vdb_root = tempfile::TempDir::new().unwrap();
    let temp_cache_root = tempfile::TempDir::new().unwrap();

    let local_packages = binpkg_root.path().join("Packages");
    tokio::fs::create_dir_all(repo_root.path().join("gentoo/.git"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(repo_root.path().join("gentoo/metadata"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(portage_root.path())
        .await
        .unwrap();
    tokio::fs::write(&local_packages, b"PKGDIR index\n")
        .await
        .unwrap();
    tokio::fs::write(
        repo_root.path().join("gentoo/.git/HEAD"),
        b"ref: refs/heads/main\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        repo_root.path().join("gentoo/not-metadata.txt"),
        b"ignore\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        repo_root.path().join("gentoo/metadata/cache.lock"),
        b"lock noise\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        repo_root.path().join("gentoo/metadata/cache.tmp"),
        b"tmp noise\n",
    )
    .await
    .unwrap();
    tokio::fs::create_dir_all(eclass_root.path().join("regen"))
        .await
        .unwrap();
    tokio::fs::write(
        eclass_root.path().join("regen/metadata.swp"),
        b"swap noise\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        binpkg_root.path().join("dev-libs-demo-1.0.gpkg.tar"),
        b"binpkg payload\n",
    )
    .await
    .unwrap();
    tokio::fs::write(portage_root.path().join("world.part"), b"partial noise\n")
        .await
        .unwrap();
    tokio::fs::write(distfiles_root.path().join("demo-1.0.tar.xz"), b"distfile\n")
        .await
        .unwrap();
    tokio::fs::create_dir_all(vdb_root.path().join("sys-apps/portage-3.0.0"))
        .await
        .unwrap();
    tokio::fs::write(
        vdb_root.path().join("sys-apps/portage-3.0.0/CONTENTS"),
        b"obj /usr/bin/emerge\n",
    )
    .await
    .unwrap();
    tokio::fs::write(temp_cache_root.path().join("temp.log"), b"temporary\n")
        .await
        .unwrap();

    let manifest = capture_final_state_parity_from(
        output.path(),
        repo_root.path(),
        eclass_root.path(),
        &local_packages,
        portage_root.path(),
    )
    .await
    .expect("capture final-state parity");

    assert!(!manifest.files.contains_key("var/db/repos/gentoo/.git/HEAD"));
    assert!(
        !manifest
            .files
            .contains_key("var/db/repos/gentoo/not-metadata.txt")
    );
    assert!(
        !manifest
            .files
            .contains_key("var/cache/binpkgs/dev-libs-demo-1.0.gpkg.tar")
    );
    assert!(
        !manifest
            .files
            .contains_key("var/db/repos/gentoo/metadata/cache.lock")
    );
    assert!(
        !manifest
            .files
            .contains_key("var/db/repos/gentoo/metadata/cache.tmp")
    );
    assert!(
        !manifest
            .files
            .contains_key("var/cache/eclass/regen/metadata.swp")
    );
    assert!(
        !manifest
            .files
            .contains_key("var/cache/distfiles/demo-1.0.tar.xz")
    );
    assert!(
        !manifest
            .files
            .contains_key("var/db/pkg/sys-apps/portage-3.0.0/CONTENTS")
    );
    assert!(!manifest.files.contains_key("var/cache/portage/temp.log"));
    assert!(!manifest.files.contains_key("var/lib/portage/world.part"));
    assert!(manifest.files.contains_key("var/cache/binpkgs/Packages"));
}

#[tokio::test]
async fn reconcile_final_state_parity_skips_up_to_date_files_and_restores_mtime() {
    let root = tempfile::TempDir::new().unwrap();
    let metadata_dir = root.path().join("var/db/repos/gentoo/metadata/md5-cache");
    tokio::fs::create_dir_all(&metadata_dir).await.unwrap();

    let reused_path = root.path().join("var/lib/portage/world");
    tokio::fs::create_dir_all(reused_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&reused_path, b"app-misc/hello\n")
        .await
        .unwrap();
    filetime::set_file_mtime(
        &reused_path,
        filetime::FileTime::from_unix_time(1_700_000_021, 0),
    )
    .unwrap();

    let restored_relative = "var/db/repos/gentoo/metadata/md5-cache/dev-libs-demo-1.0";
    let restored_digest = hex::encode(sha2::Sha256::digest(b"EAPI=8\n"));
    let reused_digest = hex::encode(sha2::Sha256::digest(b"app-misc/hello\n"));

    let requests = Arc::new(Mutex::new(Vec::new()));
    let base_url = spawn_blob_server(
        HashMap::from([(restored_digest.clone(), b"EAPI=8\n".to_vec())]),
        requests.clone(),
    )
    .await;
    let client = RemergeClient::new(&base_url).expect("client");
    let result = WorkorderResult {
        workorder_id: uuid::Uuid::nil(),
        built_packages: Vec::new(),
        failed_packages: Vec::new(),
        binhost_uri: String::new(),
        fetched_distfiles: BTreeMap::new(),
        parity_manifest: ParityManifest {
            files: std::collections::BTreeMap::from([
                (
                    "var/lib/portage/world".into(),
                    ParityFileEntry {
                        digest: reused_digest,
                        size: 15,
                        mtime_secs: 1_700_000_021,
                    },
                ),
                (
                    restored_relative.into(),
                    ParityFileEntry {
                        digest: restored_digest.clone(),
                        size: 7,
                        mtime_secs: 1_700_000_022,
                    },
                ),
            ]),
            directories: BTreeMap::new(),
            symlinks: BTreeMap::new(),
        },
    };

    reconcile_final_state_parity_into(root.path(), &client, &result)
        .await
        .expect("reconcile final-state parity");

    let restored_path = root.path().join(restored_relative);
    assert_eq!(tokio::fs::read(&restored_path).await.unwrap(), b"EAPI=8\n");
    assert_eq!(
        tokio::fs::metadata(&restored_path).await.unwrap().mtime(),
        1_700_000_022
    );
    assert_eq!(
        tokio::fs::metadata(&reused_path).await.unwrap().mtime(),
        1_700_000_021
    );
    assert_eq!(requests.lock().unwrap().clone(), vec![restored_digest]);
}

#[tokio::test]
async fn reconcile_final_state_parity_restores_directory_and_symlink_metadata() {
    let root = tempfile::TempDir::new().unwrap();
    let link_relative = "var/lib/portage/make.profile";
    let dir_relative = "var/lib/portage/sets";
    let link_target = b"../db/repos/gentoo/profiles/default/linux/amd64/23.0".to_vec();
    let link_digest = hex::encode(sha2::Sha256::digest(&link_target));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let base_url = spawn_blob_server(
        HashMap::from([(link_digest.clone(), link_target.clone())]),
        requests.clone(),
    )
    .await;
    let client = RemergeClient::new(&base_url).expect("client");
    let result = WorkorderResult {
        workorder_id: uuid::Uuid::nil(),
        built_packages: Vec::new(),
        failed_packages: Vec::new(),
        binhost_uri: String::new(),
        fetched_distfiles: BTreeMap::new(),
        parity_manifest: ParityManifest {
            files: BTreeMap::new(),
            directories: BTreeMap::from([(
                dir_relative.into(),
                ParityDirectoryEntry {
                    mtime_secs: 1_700_000_051,
                },
            )]),
            symlinks: BTreeMap::from([(
                link_relative.into(),
                ParitySymlinkEntry {
                    digest: link_digest.clone(),
                    size: link_target.len() as u64,
                    mtime_secs: 1_700_000_052,
                },
            )]),
        },
    };

    reconcile_final_state_parity_into(root.path(), &client, &result)
        .await
        .expect("reconcile parity dirs and symlinks");

    let restored_dir = root.path().join(dir_relative);
    let restored_link = root.path().join(link_relative);
    assert!(restored_dir.is_dir());
    assert!(
        std::fs::symlink_metadata(&restored_link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(&restored_link)
            .unwrap()
            .as_os_str()
            .as_bytes(),
        link_target.as_slice()
    );
    assert_eq!(
        std::fs::metadata(&restored_dir).unwrap().mtime(),
        1_700_000_051
    );
    assert_eq!(
        std::fs::symlink_metadata(&restored_link).unwrap().mtime(),
        1_700_000_052
    );
    assert_eq!(requests.lock().unwrap().clone(), vec![link_digest]);
}

#[tokio::test]
async fn reconcile_final_state_parity_accepts_zstd_encoded_blob_responses() {
    let root = tempfile::TempDir::new().unwrap();
    let restored_relative = "var/lib/portage/world";
    let restored_bytes = b"app-misc/hello\n".to_vec();
    let restored_digest = hex::encode(sha2::Sha256::digest(&restored_bytes));

    let requests = Arc::new(Mutex::new(Vec::new()));
    let base_url = spawn_zstd_blob_server(
        HashMap::from([(restored_digest.clone(), restored_bytes.clone())]),
        requests.clone(),
    )
    .await;
    let client = RemergeClient::new(&base_url).expect("client");
    let result = WorkorderResult {
        workorder_id: uuid::Uuid::nil(),
        built_packages: Vec::new(),
        failed_packages: Vec::new(),
        binhost_uri: String::new(),
        fetched_distfiles: BTreeMap::new(),
        parity_manifest: ParityManifest {
            files: std::collections::BTreeMap::from([(
                restored_relative.into(),
                ParityFileEntry {
                    digest: restored_digest.clone(),
                    size: restored_bytes.len() as u64,
                    mtime_secs: 1_700_000_041,
                },
            )]),
            directories: BTreeMap::new(),
            symlinks: BTreeMap::new(),
        },
    };

    reconcile_final_state_parity_into(root.path(), &client, &result)
        .await
        .expect("reconcile final-state parity with zstd blob response");

    assert_eq!(
        tokio::fs::read(root.path().join(restored_relative))
            .await
            .unwrap(),
        restored_bytes
    );
    assert_eq!(requests.lock().unwrap().clone(), vec![restored_digest]);
}

#[tokio::test]
async fn remerge_followup_matches_local_emerge_final_state() {
    let remerge_root = tempfile::TempDir::new().unwrap();
    let baseline_root = tempfile::TempDir::new().unwrap();
    let tools_dir = tempfile::TempDir::new().unwrap();
    let fake_emerge = tools_dir.path().join("emerge");
    write_fake_emerge_script(&fake_emerge);

    let parity_files = [
        (
            "var/lib/portage/world",
            b"app-misc/hello\n".to_vec(),
            1_700_000_031,
        ),
        (
            "var/db/repos/gentoo/metadata/md5-cache/dev-libs-demo-1.0",
            b"EAPI=8\n".to_vec(),
            1_700_000_032,
        ),
    ];

    let mut blobs = HashMap::new();
    let mut manifest_files = BTreeMap::new();
    for (relative, contents, mtime_secs) in &parity_files {
        let digest = hex::encode(sha2::Sha256::digest(contents));
        blobs.insert(digest.clone(), contents.clone());
        manifest_files.insert(
            (*relative).to_string(),
            ParityFileEntry {
                digest,
                size: contents.len() as u64,
                mtime_secs: *mtime_secs,
            },
        );

        write_file_with_mtime(&baseline_root.path().join(relative), contents, *mtime_secs);
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let base_url = spawn_blob_server(blobs, requests.clone()).await;
    let client = RemergeClient::new(&base_url).expect("client");
    let result = WorkorderResult {
        workorder_id: uuid::Uuid::nil(),
        built_packages: Vec::new(),
        failed_packages: Vec::new(),
        binhost_uri: String::new(),
        fetched_distfiles: BTreeMap::new(),
        parity_manifest: ParityManifest {
            files: manifest_files,
            directories: BTreeMap::from([
                (
                    "var/lib/portage".into(),
                    ParityDirectoryEntry {
                        mtime_secs: 1_700_000_030,
                    },
                ),
                (
                    "var/db/repos/gentoo/metadata".into(),
                    ParityDirectoryEntry {
                        mtime_secs: 1_700_000_029,
                    },
                ),
                (
                    "var/db/repos/gentoo/metadata/md5-cache".into(),
                    ParityDirectoryEntry {
                        mtime_secs: 1_700_000_028,
                    },
                ),
            ]),
            symlinks: BTreeMap::new(),
        },
    };
    let local_args = vec![
        "--getbinpkg".to_string(),
        "--usepkg".to_string(),
        "app-misc/hello".to_string(),
    ];
    let binhost_uri = "file:///tmp/remerge-test-binpkgs";

    {
        let _root_guard = common::set_root_env(remerge_root.path());
        reconcile_final_state_parity_into(remerge_root.path(), &client, &result)
            .await
            .expect("remerge parity reconcile");
        run_local_emerge_with_program(&fake_emerge, &local_args, Some(binhost_uri))
            .await
            .expect("remerge local emerge");
    }

    {
        let _root_guard = common::set_root_env(baseline_root.path());
        run_local_emerge_with_program(&fake_emerge, &local_args, Some(binhost_uri))
            .await
            .expect("baseline local emerge");
    }

    assert_eq!(
        snapshot_regular_files(remerge_root.path()),
        snapshot_regular_files(baseline_root.path())
    );
    assert_eq!(
        tokio::fs::metadata(remerge_root.path().join("var/lib/portage/world"))
            .await
            .unwrap()
            .mtime(),
        1_700_000_031
    );
    assert_eq!(
        tokio::fs::metadata(
            remerge_root
                .path()
                .join("var/db/repos/gentoo/metadata/md5-cache/dev-libs-demo-1.0"),
        )
        .await
        .unwrap()
        .mtime(),
        1_700_000_032
    );
    assert_eq!(requests.lock().unwrap().len(), parity_files.len());
}

#[tokio::test]
async fn remerge_followup_restores_fetched_distfiles_for_plain_local_emerge() {
    let remerge_root = tempfile::TempDir::new().unwrap();
    let baseline_root = tempfile::TempDir::new().unwrap();
    let tools_dir = tempfile::TempDir::new().unwrap();
    let fake_emerge = tools_dir.path().join("emerge");
    write_fake_distfile_emerge_script(&fake_emerge);

    let distfile_bytes = b"downloaded-distfile\n".to_vec();
    let distfile_digest = hex::encode(sha2::Sha256::digest(&distfile_bytes));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let base_url = spawn_blob_server(
        HashMap::from([(distfile_digest.clone(), distfile_bytes.clone())]),
        requests.clone(),
    )
    .await;
    let client = RemergeClient::new(&base_url).expect("client");
    let result = WorkorderResult {
        workorder_id: uuid::Uuid::nil(),
        built_packages: Vec::new(),
        failed_packages: Vec::new(),
        binhost_uri: String::new(),
        fetched_distfiles: BTreeMap::from([(
            "demo-1.0.tar.xz".into(),
            SnapshotEntry {
                digest: distfile_digest.clone(),
                size: distfile_bytes.len() as u64,
                mtime_secs: 1_700_000_071,
            },
        )]),
        parity_manifest: ParityManifest::default(),
    };
    let local_args = vec![
        "--getbinpkg".to_string(),
        "--usepkg".to_string(),
        "=dev-libs/demo-1.0".to_string(),
    ];
    let binhost_uri = "file:///tmp/remerge-test-binpkgs";

    write_file_with_mtime(
        &baseline_root
            .path()
            .join("var/cache/distfiles/demo-1.0.tar.xz"),
        &distfile_bytes,
        1_700_000_071,
    );

    {
        let _root_guard = common::set_root_env(remerge_root.path());
        reconcile_fetched_distfiles_into(
            &remerge_root.path().join("var/cache/distfiles"),
            &client,
            &result,
        )
        .await
        .expect("restore fetched distfiles");
        run_local_emerge_with_program(&fake_emerge, &local_args, Some(binhost_uri))
            .await
            .expect("remerge local emerge");
    }

    {
        let _root_guard = common::set_root_env(baseline_root.path());
        run_local_emerge_with_program(&fake_emerge, &local_args, Some(binhost_uri))
            .await
            .expect("baseline local emerge");
    }

    assert_eq!(
        snapshot_regular_files(remerge_root.path()),
        snapshot_regular_files(baseline_root.path())
    );
    assert_eq!(requests.lock().unwrap().clone(), vec![distfile_digest]);
}
