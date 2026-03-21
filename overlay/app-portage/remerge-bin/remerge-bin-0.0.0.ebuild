# Copyright 2026 Kristoffer Forss
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit verify-sig

MY_PN="${PN/-bin/}"

DESCRIPTION="Drop-in emerge wrapper that offloads builds to a remote binary host (pre-built)"
HOMEPAGE="https://github.com/k-forss/remerge"

SRC_URI="
	amd64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-amd64-linux.tar.gz )
	arm64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-arm64-linux.tar.gz )
	arm? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-arm-linux.tar.gz )
	ppc64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-ppc64-linux.tar.gz )
	riscv? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-riscv64-linux.tar.gz )
	verify-sig? (
		amd64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-amd64-linux.tar.gz.asc )
		arm64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-arm64-linux.tar.gz.asc )
		arm? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-arm-linux.tar.gz.asc )
		ppc64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-ppc64-linux.tar.gz.asc )
		riscv? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-riscv64-linux.tar.gz.asc )
	)
"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64 ~arm ~arm64 ~ppc64 ~riscv"
IUSE="verify-sig"

RDEPEND="
	sys-apps/portage
	!app-portage/remerge
"
BDEPEND="verify-sig? ( sec-keys/openpgp-keys-remerge )"

VERIFY_SIG_OPENPGP_KEY_PATH="/usr/share/openpgp-keys/remerge.asc"

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
	elog "Edit /etc/remerge.conf and set 'server' to your remerge build server URL."
	elog ""
	elog "  server = \"http://remerge.example.com:7654\""
	elog ""
	elog "A client_id will be generated automatically on first run."
}
