#!/usr/bin/env bash
set -euo pipefail

packages="$(realpath "$1")"
out="$(realpath "$2")"
key="$3"
url="$4"

debs=("$packages"/*.deb)
rpms=("$packages"/*.rpm)
test -e "${debs[0]}" || { echo "no .deb in $packages" >&2; exit 1; }
test -e "${rpms[0]}" || { echo "no .rpm in $packages" >&2; exit 1; }

rm -rf "$out/deb" "$out/rpm"
mkdir -p "$out/deb/pool/main" "$out/rpm"
touch "$out/.nojekyll"
gpg --batch --yes --armor --export "$key" > "$out/key.asc"
gpg --batch --yes --export "$key" > "$out/key.gpg"

release="$(mktemp)"
cp "${debs[@]}" "$out/deb/pool/main/"
(
  cd "$out/deb"
  for arch in amd64 arm64; do
    mkdir -p "dists/stable/main/binary-$arch"
    apt-ftparchive --arch "$arch" packages pool > "dists/stable/main/binary-$arch/Packages"
    gzip -9kf "dists/stable/main/binary-$arch/Packages"
  done
  apt-ftparchive \
    -o APT::FTPArchive::Release::Origin=SDR-- \
    -o APT::FTPArchive::Release::Label=SDR-- \
    -o APT::FTPArchive::Release::Suite=stable \
    -o APT::FTPArchive::Release::Codename=stable \
    -o APT::FTPArchive::Release::Architectures="amd64 arm64" \
    -o APT::FTPArchive::Release::Components=main \
    release dists/stable > "$release"
  mv "$release" dists/stable/Release
  gpg --batch --yes --local-user "$key" --clearsign -o dists/stable/InRelease dists/stable/Release
  gpg --batch --yes --local-user "$key" --armor --detach-sign -o dists/stable/Release.gpg dists/stable/Release
)

cp "${rpms[@]}" "$out/rpm/"
rpmsign --define "_gpg_name $key" --define "__gpg $(command -v gpg)" --addsign "$out"/rpm/*.rpm
createrepo_c "$out/rpm"
gpg --batch --yes --local-user "$key" --armor --detach-sign "$out/rpm/repodata/repomd.xml"
cat > "$out/rpm/sdrminusminus.repo" <<EOF
[sdrminusminus]
name=SDR--
baseurl=$url/rpm
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=$url/key.asc
EOF

find "$out" -type f -not -path '*/.git/*' | sort
