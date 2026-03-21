# Copyright 2026 Kristoffer Forss
# Distributed under the terms of the GNU General Public License v2

EAPI=8

CRATES=" "

inherit cargo git-r3 systemd

DESCRIPTION="Distributed binary package build server for Gentoo Linux"
HOMEPAGE="https://github.com/k-forss/remerge"
EGIT_REPO_URI="https://github.com/k-forss/remerge.git"

LICENSE="GPL-2"
SLOT="0"
PROPERTIES="live"

RDEPEND="
	app-containers/docker
	!app-portage/remerge-server-bin
"

QA_FLAGS_IGNORED="usr/bin/remerge-server"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_configure() {
	cargo_src_configure
}

src_compile() {
	cargo_src_compile --package remerge-server
}

src_test() {
	cargo_src_test --package remerge-server --package remerge-types
}

src_install() {
	cargo_src_install --path crates/server

	# Configuration
	insinto /etc/remerge
	newins config/server.example.toml server.toml

	# Directories
	keepdir /var/cache/remerge/binpkgs
	fowners root:root /var/cache/remerge/binpkgs
	fperms 0755 /var/cache/remerge/binpkgs

	# OpenRC
	newinitd "${FILESDIR}"/remerge-server.initd remerge-server
	newconfd "${FILESDIR}"/remerge-server.confd remerge-server

	# systemd
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
