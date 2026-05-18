use std::collections::BTreeMap;
use std::path::PathBuf;

use tempfile::TempDir;

use remerge_types::portage::*;

/// Create a temp directory with a populated `/etc/portage/` tree.
/// Returns (TempDir, PathBuf) — keep TempDir alive for the test duration.
pub fn portage_tree() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let root = tmp.path().to_path_buf();
    let portage = root.join("etc/portage");
    std::fs::create_dir_all(&portage).unwrap();

    // Write a minimal make.conf
    std::fs::write(
        portage.join("make.conf"),
        r#"CFLAGS="-O2 -pipe"
CXXFLAGS="${CFLAGS}"
LDFLAGS="-Wl,-O1 -Wl,--as-needed"
MAKEOPTS="-j4"
USE="X wayland -systemd"
FEATURES="buildpkg noclean"
ACCEPT_LICENSE="-* @FREE"
ACCEPT_KEYWORDS="amd64"
EMERGE_DEFAULT_OPTS=""
CHOST="x86_64-pc-linux-gnu"
VIDEO_CARDS="amdgpu"
INPUT_DEVICES="libinput"
"#,
    )
    .unwrap();

    // Create directory structure
    std::fs::create_dir_all(portage.join("package.use")).unwrap();
    std::fs::create_dir_all(portage.join("package.accept_keywords")).unwrap();
    std::fs::create_dir_all(portage.join("package.license")).unwrap();
    std::fs::create_dir_all(portage.join("package.mask")).unwrap();
    std::fs::create_dir_all(portage.join("package.unmask")).unwrap();
    std::fs::create_dir_all(portage.join("package.env")).unwrap();
    std::fs::create_dir_all(portage.join("env")).unwrap();
    std::fs::create_dir_all(portage.join("repos.conf")).unwrap();
    std::fs::create_dir_all(portage.join("profile")).unwrap();
    std::fs::create_dir_all(portage.join("patches")).unwrap();

    // Write package.use entries
    std::fs::write(
        portage.join("package.use/custom"),
        "dev-libs/openssl -bindist\nsys-apps/systemd cryptsetup\n",
    )
    .unwrap();

    // Write package.accept_keywords
    std::fs::write(
        portage.join("package.accept_keywords/custom"),
        "sys-kernel/gentoo-sources ~amd64\n",
    )
    .unwrap();

    // Write package.license
    std::fs::write(
        portage.join("package.license/custom"),
        "sys-kernel/linux-firmware linux-fw-redistributable\n",
    )
    .unwrap();

    // Write package.mask
    std::fs::write(portage.join("package.mask/custom"), ">=dev-libs/foo-2.0\n").unwrap();

    // Write package.unmask
    std::fs::write(portage.join("package.unmask/custom"), "=dev-libs/bar-1.5\n").unwrap();

    // Write package.env
    std::fs::write(
        portage.join("package.env/custom"),
        "dev-qt/qtwebengine no-lto.conf\n",
    )
    .unwrap();

    // Write env file
    std::fs::write(
        portage.join("env/no-lto.conf"),
        "CFLAGS=\"${CFLAGS} -fno-lto\"\nCXXFLAGS=\"${CFLAGS}\"\n",
    )
    .unwrap();

    // Write repos.conf
    std::fs::write(
        portage.join("repos.conf/gentoo.conf"),
        "[gentoo]\nlocation = /var/db/repos/gentoo\nsync-type = rsync\nsync-uri = rsync://rsync.gentoo.org/gentoo-portage\n",
    )
    .unwrap();

    // Write profile overlay
    std::fs::write(portage.join("profile/use.mask"), "custom-flag\n").unwrap();

    // Write patches
    let patch_dir = portage.join("patches/dev-libs/openssl");
    std::fs::create_dir_all(&patch_dir).unwrap();
    std::fs::write(
        patch_dir.join("fix.patch"),
        "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n",
    )
    .unwrap();

    // Write world file
    let var_lib = root.join("var/lib/portage");
    std::fs::create_dir_all(&var_lib).unwrap();
    std::fs::write(
        var_lib.join("world"),
        "dev-libs/openssl\nsys-apps/systemd\napp-misc/screen\n",
    )
    .unwrap();

    (tmp, root)
}

/// Create a temp directory with a local overlay repo and matching distfiles.
///
/// Returns `(TempDir, root, overlay_name, distfile_name)`.
pub fn portage_tree_with_local_overlay() -> (TempDir, PathBuf, String, String) {
    let (tmp, root) = portage_tree();
    let portage = root.join("etc/portage");

    let overlay_name = "local-overlay".to_string();
    let distfile_name = "demo-1.0.tar.xz".to_string();
    let overlay_root = root.join("var/db/repos").join(&overlay_name);
    let package_dir = overlay_root.join("dev-libs/demo");
    let metadata_dir = package_dir.join("metadata");
    std::fs::create_dir_all(&metadata_dir).unwrap();
    std::fs::create_dir_all(overlay_root.join("profiles")).unwrap();

    std::fs::write(
        overlay_root.join("profiles/repo_name"),
        format!("{overlay_name}\n"),
    )
    .unwrap();
    std::fs::write(
        package_dir.join("demo-1.0.ebuild"),
        "EAPI=8\nDESCRIPTION=\"demo\"\nSRC_URI=\"https://example.invalid/demo-1.0.tar.xz\"\nSLOT=\"0\"\nKEYWORDS=\"~amd64\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("Manifest"),
        format!("DIST {distfile_name} 12 BLAKE2B deadbeef SHA512 cafefood\n"),
    )
    .unwrap();
    std::fs::write(metadata_dir.join("layout.conf"), "masters = gentoo\n").unwrap();
    std::fs::create_dir_all(overlay_root.join(".git")).unwrap();
    std::fs::write(overlay_root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

    std::fs::write(
        portage.join("repos.conf/local-overlay.conf"),
        format!(
            "[local-overlay]\nlocation = {}\nauto-sync = no\nmasters = gentoo\n",
            overlay_root.display()
        ),
    )
    .unwrap();

    let distdir = root.join("var/cache/distfiles");
    std::fs::create_dir_all(&distdir).unwrap();
    std::fs::write(distdir.join(&distfile_name), b"demo-distfile").unwrap();

    (tmp, root, overlay_name, distfile_name)
}

/// Create a temp directory with both `/etc/portage/make.conf` and
/// `/etc/make.conf` populated so tests can verify merged path behavior.
pub fn portage_tree_with_dual_make_conf() -> (TempDir, PathBuf) {
    let (tmp, root) = portage_tree();

    std::fs::create_dir_all(root.join("etc")).unwrap();
    std::fs::write(
        root.join("etc/make.conf"),
        r#"CFLAGS="-O3 -pipe"
ACCEPT_KEYWORDS="~amd64"
FEATURES="buildpkg test"
CUSTOM_LEGACY="yes"
"#,
    )
    .unwrap();

    std::fs::write(
        root.join("etc/portage/make.conf"),
        r#"CFLAGS="-O2 -pipe"
CXXFLAGS="${CFLAGS}"
LDFLAGS="-Wl,-O1 -Wl,--as-needed"
MAKEOPTS="-j4"
USE="X wayland -systemd"
FEATURES="buildpkg noclean"
ACCEPT_LICENSE="-* @FREE"
ACCEPT_KEYWORDS="amd64"
EMERGE_DEFAULT_OPTS="--quiet"
CHOST="x86_64-pc-linux-gnu"
VIDEO_CARDS="amdgpu"
INPUT_DEVICES="libinput"
CUSTOM_PORTAGE="yes"
"#,
    )
    .unwrap();

    (tmp, root)
}

/// Add nested directory entries under package.use and package.accept_keywords.
pub fn portage_tree_with_nested_package_dirs() -> (TempDir, PathBuf) {
    let (tmp, root) = portage_tree();
    let portage = root.join("etc/portage");

    let nested_use_dir = portage.join("package.use/nested/deeper");
    std::fs::create_dir_all(&nested_use_dir).unwrap();
    std::fs::write(
        nested_use_dir.join("extra"),
        "media-libs/mesa llvm\nsys-libs/zlib minizip\n",
    )
    .unwrap();

    let nested_keywords_dir = portage.join("package.accept_keywords/nested/deeper");
    std::fs::create_dir_all(&nested_keywords_dir).unwrap();
    std::fs::write(nested_keywords_dir.join("extra"), "dev-util/re2c ~amd64\n").unwrap();

    (tmp, root)
}

/// Overwrite the fixture's global ACCEPT_KEYWORDS and package.accept_keywords
/// content for empty-entry semantics tests.
pub fn portage_tree_with_empty_accept_keywords(global_accept_keywords: &str) -> (TempDir, PathBuf) {
    let (tmp, root) = portage_tree();
    let portage = root.join("etc/portage");

    std::fs::write(
        portage.join("make.conf"),
        format!(
            "CFLAGS=\"-O2 -pipe\"\nCXXFLAGS=\"${{CFLAGS}}\"\nLDFLAGS=\"-Wl,-O1 -Wl,--as-needed\"\nMAKEOPTS=\"-j4\"\nUSE=\"X wayland -systemd\"\nFEATURES=\"buildpkg noclean\"\nACCEPT_LICENSE=\"-* @FREE\"\nACCEPT_KEYWORDS=\"{}\"\nEMERGE_DEFAULT_OPTS=\"\"\nCHOST=\"x86_64-pc-linux-gnu\"\nVIDEO_CARDS=\"amdgpu\"\nINPUT_DEVICES=\"libinput\"\n",
            global_accept_keywords
        ),
    )
    .unwrap();

    std::fs::write(
        portage.join("package.accept_keywords/custom"),
        "sys-kernel/gentoo-sources\n",
    )
    .unwrap();

    (tmp, root)
}

/// Create a temp directory with a populated `/var/db/pkg/` VDB.
pub fn vdb_tree(packages: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let root = tmp.path().to_path_buf();

    for (category, name_version) in packages {
        let pkg_dir = root.join("var/db/pkg").join(category).join(name_version);
        std::fs::create_dir_all(&pkg_dir).unwrap();
        // Create a minimal PF file so the directory is recognized as a valid package
        std::fs::write(pkg_dir.join("PF"), name_version).unwrap();
    }

    (tmp, root)
}

/// Build a minimal PortageConfig with sensible defaults.
pub fn minimal_portage_config() -> PortageConfig {
    PortageConfig {
        make_conf: MakeConf::default(),
        package_use: Vec::new(),
        package_accept_keywords: Vec::new(),
        package_license: Vec::new(),
        package_mask: Vec::new(),
        package_unmask: Vec::new(),
        package_env: Vec::new(),
        env_files: BTreeMap::new(),
        repos_conf: BTreeMap::new(),
        snapshot_manifest: Default::default(),
        repo_snapshots: BTreeMap::new(),
        repo_snapshot_refs: BTreeMap::new(),
        repo_snapshot_trees: BTreeMap::new(),
        patches: BTreeMap::new(),
        distfile_snapshots: BTreeMap::new(),
        distfile_snapshot_refs: BTreeMap::new(),
        profile_overlay: BTreeMap::new(),
        profile: "default/linux/amd64/23.0".into(),
        world: Vec::new(),
    }
}

/// Build a fully-populated PortageConfig for comprehensive testing.
pub fn full_portage_config() -> PortageConfig {
    let mut use_expand = BTreeMap::new();
    use_expand.insert(
        "VIDEO_CARDS".to_string(),
        vec!["intel".to_string(), "amdgpu".to_string()],
    );
    use_expand.insert("INPUT_DEVICES".to_string(), vec!["libinput".to_string()]);

    let mut extra = BTreeMap::new();
    extra.insert(
        "GENTOO_MIRRORS".to_string(),
        "https://mirror.example.com/gentoo".to_string(),
    );

    let mut repos_conf = BTreeMap::new();
    repos_conf.insert(
        "gentoo.conf".to_string(),
        "[gentoo]\nlocation = /var/db/repos/gentoo\nsync-type = rsync\n".to_string(),
    );

    let mut repo_snapshots = BTreeMap::new();
    repo_snapshots.insert(
        "local-overlay".to_string(),
        BTreeMap::from([(
            "dev-libs/demo/demo-1.0.ebuild".to_string(),
            "EAPI=8\nDESCRIPTION=\"demo\"\n".to_string(),
        )]),
    );

    let mut patches = BTreeMap::new();
    patches.insert(
        "dev-libs/openssl/fix.patch".to_string(),
        "--- a/file\n+++ b/file\n".to_string(),
    );

    let mut profile_overlay = BTreeMap::new();
    profile_overlay.insert("use.mask".to_string(), "custom-flag\n".to_string());
    profile_overlay.insert(
        "package.provided".to_string(),
        "sys-libs/glibc-2.38\n".to_string(),
    );

    let mut env_files = BTreeMap::new();
    env_files.insert(
        "no-lto.conf".to_string(),
        "CFLAGS=\"-fno-lto\"\n".to_string(),
    );

    let mut distfile_snapshots = BTreeMap::new();
    distfile_snapshots.insert("demo-1.0.tar.xz".to_string(), b"demo-distfile".to_vec());

    PortageConfig {
        make_conf: MakeConf {
            cflags: "-O2 -pipe -march=skylake".to_string(),
            cxxflags: "${CFLAGS}".to_string(),
            ldflags: "-Wl,-O1 -Wl,--as-needed".to_string(),
            makeopts: "-j12".to_string(),
            use_flags: vec![
                "X".into(),
                "wayland".into(),
                "dbus".into(),
                "-systemd".into(),
            ],
            features: vec!["buildpkg".into(), "noclean".into(), "parallel-fetch".into()],
            accept_license: "-* @FREE".to_string(),
            accept_keywords: "~amd64".to_string(),
            emerge_default_opts: "--verbose --keep-going".to_string(),
            chost: "x86_64-pc-linux-gnu".to_string(),
            cpu_flags: Some((
                "CPU_FLAGS_X86".to_string(),
                vec![
                    "aes".into(),
                    "avx".into(),
                    "avx2".into(),
                    "sse".into(),
                    "sse2".into(),
                ],
            )),
            original_cflags: Some("-O2 -pipe -march=native".to_string()),
            use_expand,
            extra,
            use_flags_resolved: true,
        },
        package_use: vec![
            PackageUseEntry {
                atom: "dev-libs/openssl".into(),
                flags: vec!["-bindist".into()],
            },
            PackageUseEntry {
                atom: "sys-apps/systemd".into(),
                flags: vec!["cryptsetup".into()],
            },
        ],
        package_accept_keywords: vec![PackageKeywordEntry {
            atom: "sys-kernel/gentoo-sources".into(),
            keywords: vec!["~amd64".into()],
        }],
        package_license: vec![PackageLicenseEntry {
            atom: "sys-kernel/linux-firmware".into(),
            licenses: vec!["linux-fw-redistributable".into()],
        }],
        package_mask: vec![">=dev-libs/foo-2.0".into()],
        package_unmask: vec!["=dev-libs/bar-1.5".into()],
        package_env: vec![PackageEnvEntry {
            atom: "dev-qt/qtwebengine".into(),
            env_file: "no-lto.conf".into(),
        }],
        env_files,
        repos_conf,
        snapshot_manifest: Default::default(),
        repo_snapshots,
        repo_snapshot_refs: BTreeMap::new(),
        repo_snapshot_trees: BTreeMap::new(),
        patches,
        distfile_snapshots,
        distfile_snapshot_refs: BTreeMap::new(),
        profile_overlay,
        profile: "default/linux/amd64/23.0".into(),
        world: vec!["dev-libs/openssl".into(), "sys-apps/systemd".into()],
    }
}

/// Build a minimal SystemIdentity with sensible defaults.
pub fn minimal_system_identity() -> SystemIdentity {
    SystemIdentity {
        arch: "amd64".into(),
        chost: "x86_64-pc-linux-gnu".into(),
        gcc_version: "13.2.0".into(),
        libc_version: "2.38".into(),
        kernel_version: "6.6.0".into(),
        python_targets: vec!["python3_12".into()],
        profile: "default/linux/amd64/23.0".into(),
    }
}

/// Build a cross-architecture SystemIdentity (aarch64) for crossdev tests.
pub fn cross_arch_system_identity() -> SystemIdentity {
    SystemIdentity {
        arch: "arm64".into(),
        chost: "aarch64-unknown-linux-gnu".into(),
        gcc_version: "13.2.0".into(),
        libc_version: "2.38".into(),
        kernel_version: "6.6.0".into(),
        python_targets: vec!["python3_12".into()],
        profile: "default/linux/arm64/23.0".into(),
    }
}
