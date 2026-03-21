# Copyright 2026 Kristoffer Forss
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit verify-sig systemd

MY_PN="${PN/-bin/}"

DESCRIPTION="Distributed binary package build server for Gentoo Linux (pre-built)"
HOMEPAGE="https://github.com/k-forss/remerge"

SRC_URI="
	amd64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-amd64-linux.tar.gz )
	arm64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-arm64-linux.tar.gz )
	verify-sig? (
		amd64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-amd64-linux.tar.gz.asc )
		arm64? ( https://github.com/k-forss/remerge/releases/download/v${PV}/${MY_PN}-v${PV}-arm64-linux.tar.gz.asc )
	)
"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
IUSE="verify-sig"

RDEPEND="
	app-containers/docker
	!app-portage/remerge-server
"
BDEPEND="verify-sig? ( sec-keys/openpgp-keys-remerge )"

VERIFY_SIG_OPENPGP_KEY_PATH="/usr/share/openpgp-keys/remerge.asc"

QA_PREBUILT="usr/bin/remerge-server"

S="${WORKDIR}"

src_install() {
	exeinto /usr/bin
	doexe remerge-server

	insinto /etc/remerge
	newins - server.toml <<-EOF
	# remerge server configuration
	#
	# See https://github.com/k-forss/remerge for all options.

	binpkg_dir = "/var/cache/remerge/binpkgs"
	binhost_url = "http://localhost:7654/binpkgs"
	docker_socket = "unix:///var/run/docker.sock"
	worker_image_prefix = "remerge-worker"
	max_workers = 4
	worker_idle_timeout = 3600
	worker_binpkg_mount = "/var/cache/binpkgs"
	EOF

	keepdir /var/cache/remerge/binpkgs
	fowners root:root /var/cache/remerge/binpkgs
	fperms 0755 /var/cache/remerge/binpkgs

	newinitd "${FILESDIR}"/remerge-server.initd remerge-server
	newconfd "${FILESDIR}"/remerge-server.confd remerge-server
	systemd_dounit "${FILESDIR}"/remerge-server.service
}

pkg_postinst() {
	elog "Configure the server in /etc/remerge/server.toml"
	elog ""
	elog "Start with OpenRC:"
	elog "  rc-service remerge-server start"
	elog "  rc-update add remerge-server default"
	elog ""
	elog "Start with systemd:"
	elog "  systemctl enable --now remerge-server"
}
