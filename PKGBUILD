pkgname=tailash
pkgver=0.1.2
pkgrel=1
pkgdesc="Linux 桌面端的 mihomo 代理客户端"
arch=(x86_64)
url="https://github.com/asinkLuno/tailash"
license=('MIT')
depends=()
source=(
    "tailash"
    "tailash.desktop"
    "tailash.svg"
)

package() {
    install -Dm755 "tailash" "$pkgdir/usr/bin/tailash"
    install -Dm644 "tailash.desktop" "$pkgdir/usr/share/applications/tailash.desktop"
    install -Dm644 "tailash.svg" "$pkgdir/usr/share/icons/hicolor/scalable/apps/tailash.svg"
}
