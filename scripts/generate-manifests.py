#!/usr/bin/env python3
"""Generate Gentoo Manifest files for versioned remerge overlay ebuilds.

Computes BLAKE2B and SHA512 checksums for all distfiles referenced by
the versioned source and binary ebuilds.

Run after a GitHub Release has been published so all distfiles
(source tarball, crates, binary assets) are downloadable.

Usage:
    python3 scripts/generate-manifests.py <version>
    # e.g. python3 scripts/generate-manifests.py 0.0.0
"""

from __future__ import annotations

import hashlib
import re
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

OVERLAY = Path(__file__).resolve().parent.parent / "overlay"
REPO = "k-forss/remerge"

CRATE_DL = "https://crates.io/api/v1/crates/{name}/{ver}/download"
GITHUB_ARCHIVE = (
    f"https://github.com/{REPO}/archive/refs/tags/v{{ver}}.tar.gz"
)
GITHUB_RELEASE_ASSET = (
    f"https://github.com/{REPO}/releases/download/v{{ver}}/{{filename}}"
)


# ── helpers ───────────────────────────────────────────────────────────


def download(url: str, retries: int = 3) -> bytes:
    """Download a URL with retries.  Returns the response body."""
    req = urllib.request.Request(
        url, headers={"User-Agent": "remerge-manifest/1.0"}
    )
    for attempt in range(1, retries + 1):
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                return resp.read()
        except (urllib.error.URLError, TimeoutError):
            if attempt == retries:
                raise
    raise RuntimeError("unreachable")


def manifest_line(filename: str, data: bytes) -> str:
    """Format a single DIST line for a Manifest file."""
    b2 = hashlib.blake2b(data).hexdigest()
    s5 = hashlib.sha512(data).hexdigest()
    return f"DIST {filename} {len(data)} BLAKE2B {b2} SHA512 {s5}"


def write_manifest(ebuild_dir: Path, lines: list[str]) -> None:
    """Sort and write DIST lines to a Manifest file."""
    lines.sort()
    manifest = ebuild_dir / "Manifest"
    manifest.write_text("\n".join(lines) + "\n")
    print(f"  ✓ Wrote {manifest.relative_to(OVERLAY)} ({len(lines)} entries)")


# ── ebuild parsers ────────────────────────────────────────────────────


def parse_crates(ebuild: Path) -> list[tuple[str, str]]:
    """Extract (name, version) pairs from CRATES="..." in an ebuild."""
    content = ebuild.read_text()
    m = re.search(r'CRATES="([^"]*)"', content, re.DOTALL)
    if not m:
        return []
    crates = []
    for line in m.group(1).strip().splitlines():
        entry = line.strip()
        if not entry:
            continue
        name, _, ver = entry.partition("@")
        if ver:
            crates.append((name, ver))
    return crates


def parse_bin_filenames(ebuild: Path, version: str) -> list[str]:
    """Extract distfile filenames from a binary ebuild's SRC_URI.

    The ebuild uses unexpanded bash variables (``${MY_PN}``, ``${PV}``),
    so we match the arch slugs from those patterns and reconstruct the
    concrete filenames.

    Returns ``.tar.gz`` filenames (not ``.asc``) for all architectures.
    """
    content = ebuild.read_text()

    # Determine MY_PN from the package name: ${PN/-bin/}
    pkg_name = ebuild.stem.removesuffix(f"-{version}")  # e.g. "remerge-bin"
    my_pn = pkg_name.replace("-bin", "")  # e.g. "remerge"

    # Match arch slugs from ${MY_PN}-v${PV}-<arch>-linux.tar.gz
    arch_re = re.compile(
        r"\$\{MY_PN\}-v\$\{PV\}-(\S+?)-linux\.tar\.gz"
    )
    arches = list(dict.fromkeys(arch_re.findall(content)))

    return [f"{my_pn}-v{version}-{arch}-linux.tar.gz" for arch in arches]


# ── generators ────────────────────────────────────────────────────────


def generate_source_manifest(
    version: str,
    ebuild_dir: Path,
    crates: list[tuple[str, str]],
) -> None:
    """Generate Manifest for a source ebuild (GitHub tarball + crates)."""
    lines: list[str] = []

    # GitHub source tarball
    filename = f"remerge-{version}.tar.gz"
    url = GITHUB_ARCHIVE.format(ver=version)
    print(f"  ↓ {filename}")
    lines.append(manifest_line(filename, download(url)))

    # Crate tarballs (parallel)
    print(f"  ↓ {len(crates)} crates (16 parallel)...")
    failed: list[str] = []

    def dl_crate(entry: tuple[str, str]) -> str | None:
        name, ver = entry
        fname = f"{name}-{ver}.crate"
        url = CRATE_DL.format(name=name, ver=ver)
        try:
            return manifest_line(fname, download(url))
        except Exception as exc:
            failed.append(f"{fname}: {exc}")
            return None

    with ThreadPoolExecutor(max_workers=16) as pool:
        futures = [pool.submit(dl_crate, c) for c in crates]
        for future in as_completed(futures):
            result = future.result()
            if result:
                lines.append(result)

    if failed:
        print(f"  ⚠ {len(failed)} crate downloads failed:")
        for f in failed[:5]:
            print(f"    {f}")
        if len(failed) > 5:
            print(f"    ... and {len(failed) - 5} more")
        sys.exit(1)

    write_manifest(ebuild_dir, lines)


def generate_bin_manifest(version: str, ebuild_dir: Path) -> None:
    """Generate Manifest for a binary ebuild (release assets)."""
    ebuilds = list(ebuild_dir.glob(f"*-{version}.ebuild"))
    if not ebuilds:
        return
    filenames = parse_bin_filenames(ebuilds[0], version)

    lines: list[str] = []
    for filename in filenames:
        url = GITHUB_RELEASE_ASSET.format(ver=version, filename=filename)
        print(f"  ↓ {filename}")
        try:
            lines.append(manifest_line(filename, download(url)))
        except urllib.error.HTTPError as exc:
            print(f"  ⚠ Skipping {filename}: {exc}")

    # Also try .asc signature files (may not exist if signing hasn't
    # run yet — skip gracefully, user can regenerate after signing).
    for filename in filenames:
        asc = filename + ".asc"
        url = GITHUB_RELEASE_ASSET.format(ver=version, filename=asc)
        try:
            lines.append(manifest_line(asc, download(url)))
        except urllib.error.HTTPError:
            pass  # signing hasn't completed yet — that's fine

    write_manifest(ebuild_dir, lines)


# ── main ──────────────────────────────────────────────────────────────


def main() -> None:
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <version>", file=sys.stderr)
        sys.exit(1)

    version = sys.argv[1]
    print(f"Generating overlay Manifest files for v{version}\n")

    # ── Source ebuilds ──
    src_cli_dir = OVERLAY / "app-portage" / "remerge"
    src_cli_ebuild = src_cli_dir / f"remerge-{version}.ebuild"

    if src_cli_ebuild.exists():
        crates = parse_crates(src_cli_ebuild)
        print(f"[remerge-{version}]  (source, {len(crates)} crates)")
        generate_source_manifest(version, src_cli_dir, crates)

        # Server shares the exact same SRC_URI — copy the Manifest
        # instead of downloading 283 crates a second time.
        src_srv_dir = OVERLAY / "app-portage" / "remerge-server"
        src_srv_ebuild = src_srv_dir / f"remerge-server-{version}.ebuild"
        if src_srv_ebuild.exists():
            import shutil

            shutil.copy2(src_cli_dir / "Manifest", src_srv_dir / "Manifest")
            print(f"\n[remerge-server-{version}]  (source, copied Manifest)")
            print(f"  ✓ Same distfiles as remerge — copied")
    else:
        print(f"  (no source ebuild for {version}, skipping)")

    # ── Binary ebuilds ──
    for pkg in ("remerge-bin", "remerge-server-bin"):
        pkg_dir = OVERLAY / "app-portage" / pkg
        ebuild = pkg_dir / f"{pkg}-{version}.ebuild"
        if ebuild.exists():
            print(f"\n[{pkg}-{version}]  (binary)")
            generate_bin_manifest(version, pkg_dir)

    print("\n✓ Done")


if __name__ == "__main__":
    main()
