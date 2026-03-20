# Copyright 2026 Kristoffer Forss
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="OpenPGP key used to sign remerge release artifacts"
HOMEPAGE="https://github.com/k-forss/remerge"

S="${WORKDIR}"

LICENSE="public-domain"
SLOT="0"
KEYWORDS="~amd64 ~arm ~arm64 ~ppc64 ~riscv"

src_install() {
	insinto /usr/share/openpgp-keys
	newins "${FILESDIR}"/remerge-release.asc remerge.asc
}
