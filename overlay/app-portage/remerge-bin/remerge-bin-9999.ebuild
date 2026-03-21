# Copyright 2026 Kristoffer Forss
# Distributed under the terms of the GNU General Public License v2

EAPI=8

MY_PN="${PN/-bin/}"
NIGHTLY_BASE="https://github.com/k-forss/remerge/releases/download/nightly"

DESCRIPTION="Drop-in emerge wrapper that offloads builds to a remote binary host (pre-built nightly)"
HOMEPAGE="https://github.com/k-forss/remerge"

# No SRC_URI — fetched in src_unpack() so no Manifest is needed for
# this rolling live ebuild.
SRC_URI=""

LICENSE="GPL-2"
SLOT="0"
PROPERTIES="live"
RESTRICT="strip"

RDEPEND="
	sys-apps/portage
	!app-portage/remerge
"
BDEPEND="net-misc/wget"

QA_PREBUILT="usr/bin/remerge"

S="${WORKDIR}"

src_unpack() {
	local arch
	case ${ARCH} in
		amd64) arch="amd64" ;;
		arm64) arch="arm64" ;;
		*) die "Unsupported architecture: ${ARCH}" ;;
	esac

	local tarball="${MY_PN}-nightly-${arch}-linux.tar.gz"
	einfo "Downloading ${tarball} from nightly release…"
	wget -q -O "${T}/${tarball}" "${NIGHTLY_BASE}/${tarball}" \
		|| die "Failed to download nightly binary"

	cd "${WORKDIR}" || die
	tar xzf "${T}/${tarball}" || die "Failed to extract tarball"
}

src_install() {
	exeinto /usr/bin
	doexe remerge

	insinto /etc
	newins - remerge.conf <<-EOF
	# remerge CLI configuration
	#
	# server: URL of the remerge build server.
	# client_id: auto-generated unique identifier for this machine.

	server = "http://localhost:7654"
	# client_id is generated automatically on first run.
	EOF
}

pkg_postinst() {
	elog "This is a nightly build from the main branch."
	elog "To update, re-emerge:"
	elog ""
	elog "  emerge -1 app-portage/remerge-bin"
	elog ""
	elog "Edit /etc/remerge.conf and set 'server' to your remerge build server URL."
	elog ""
	elog "  server = \"http://remerge.example.com:7654\""
	elog ""
	elog "A client_id will be generated automatically on first run."
}
