# Copyright 2026 Kristoffer Forss
# Distributed under the terms of the GNU General Public License v2

EAPI=8

CRATES=" "

inherit cargo git-r3

DESCRIPTION="Drop-in emerge wrapper that offloads builds to a remote binary host"
HOMEPAGE="https://github.com/k-forss/remerge"
EGIT_REPO_URI="https://github.com/k-forss/remerge.git"

LICENSE="GPL-2"
SLOT="0"
PROPERTIES="live"

RDEPEND="
	!app-portage/remerge-bin
	sys-apps/portage
"
BDEPEND="virtual/rust"

QA_FLAGS_IGNORED="usr/bin/remerge"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_configure() {
	cargo_src_configure
}

src_compile() {
	cargo_src_compile --package remerge
}

src_test() {
	cargo_src_test --package remerge --package remerge-types
}

src_install() {
	cargo_src_install --path crates/cli

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
	elog "Edit /etc/remerge.conf and set 'server' to your remerge build server URL."
	elog ""
	elog "  server = \"http://remerge.example.com:7654\""
	elog ""
	elog "A client_id will be generated automatically on first run."
}
