# Maintainer: asinkLuno <asinkLuno@users.noreply.github.com>
# tailash 的 pacman 包：CI 打包脚本按本文件打 .pkg.tar.zst 并发布到 GitHub Release。
# 非 AUR 包：用户从 Release 页下载后 pacman -U 直接安装。
# CI 用法见 .github/workflows/release.yml（arch 打包 job）。
# 本地验证：在仓库根目录 updpkgsums / makepkg -f（需要先提供 src/tailash）。

pkgname=tailash
pkgver=0.1.0
pkgrel=1
pkgdesc="mihomo GUI 客户端（GPUI）"
arch=('x86_64')
url="https://github.com/asinkLuno/tailash"
license=('GPL-3.0-only')
# 运行期共享库（不含 mihomo 内核，用户自行安装 mihomo/mihomo-bin）
depends=('libxkbcommon' 'libxkbcommon-x11' 'fontconfig' 'libxcb' 'wayland' 'vulkan-icd-loader')
optdepends=('mihomo: mihomo 内核（或 mihomo-bin）'
            'libcap: setcap 授权 TUN 模式所需')
provides=('tailash')
conflicts=('tailash')
source=("tailash-${pkgver}-linux-${CARCH}.tar.gz")
sha256sums=('SKIP')
options=(!strip)  # CI 已 cargo strip，二次 strip 无必要

package() {
    cd "${srcdir}/tailash-${pkgver}-linux-x86_64"
    install -Dm755 tailash "${pkgdir}/usr/bin/tailash"
    install -Dm644 LICENSE "${pkgdir}/usr/share/licenses/${pkgname}/LICENSE"
    install -Dm644 README.md "${pkgdir}/usr/share/doc/${pkgname}/README.md"
}
