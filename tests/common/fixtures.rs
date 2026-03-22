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
        patches: BTreeMap::new(),
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
        patches,
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
