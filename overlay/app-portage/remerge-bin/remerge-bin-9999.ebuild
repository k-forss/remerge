# Copyright 2026 Kristoffer Forss
# Distributed under the terms of the GNU General Public License v2

EAPI=8

MY_PN="${PN/-bin/}"

DESCRIPTION="Drop-in emerge wrapper that offloads builds to a remote binary host (pre-built nightly)"
HOMEPAGE="https://github.com/k-forss/remerge"

SRC_URI="
	amd64? ( https://github.com/k-forss/remerge/releases/download/nightly/${MY_PN}-nightly-amd64-linux.tar.gz )
	arm64? ( https://github.com/k-forss/remerge/releases/download/nightly/${MY_PN}-nightly-arm64-linux.tar.gz )
"

LICENSE="GPL-2"
SLOT="0"
PROPERTIES="live"

RDEPEND="
	sys-apps/portage
	!app-portage/remerge
"

QA_PREBUILT="usr/bin/remerge"

S="${WORKDIR}"

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
	elog "To update, clear the cached distfile and re-emerge:"
	elog ""
	elog "  rm /var/cache/distfiles/remerge-nightly-*.tar.gz"
	elog "  emerge -1 app-portage/remerge-bin"
	elog ""
	elog "Edit /etc/remerge.conf and set 'server' to your remerge build server URL."
	elog ""
	elog "  server = \"http://remerge.example.com:7654\""
	elog ""
	elog "A client_id will be generated automatically on first run."
}
