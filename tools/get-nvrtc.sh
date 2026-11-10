#!/bin/sh
# get-nvrtc.sh: fetches NVRTC, the free compiler acc needs to use an NVIDIA card, and puts it where acc looks for it.
#
# NOT TESTED ON LINUX YET. It does what tools/get-nvrtc.ps1 does on Windows, which has been tested; if something
# goes wrong here, please report it with the output.
#
# NVRTC is NVIDIA's CUDA runtime compiler: acc compiles its GPU code with it each time a run starts. It is NVIDIA's
# software, under NVIDIA's licence (the CUDA Toolkit end user licence agreement, which comes with it, in
# nvidia_cuda_nvrtc_cu12-12.9.86.dist-info/licenses/License.txt). It isn't part of this project and isn't in its
# repository. NVIDIA publishes it on PyPI, the Python package index, as the package nvidia-cuda-nvrtc-cu12.
#
# This script asks PyPI for the Linux file of version 12.9.86 for this kind of CPU and its SHA-256, downloads it
# (into .tools/downloads), checks it, unpacks it into .tools/cuda-nvrtc-12.9.86 in the project's folder, and adds
# the libnvrtc.so link that acc's CUDA library looks for. acc finds it there: it starts itself again with that folder
# on LD_LIBRARY_PATH. Nothing else on the computer is changed. It needs curl or wget, and python3 (to read PyPI's
# answer and to unpack the file, which is a zip archive).
#
#   sh tools/get-nvrtc.sh                                  download, check and unpack
#   sh tools/get-nvrtc.sh --wheel FILE                     unpack a file you downloaded, checked against PyPI's hash
#   sh tools/get-nvrtc.sh --wheel FILE --sha256 HASH       the same, checked against HASH, without the network
#   sh tools/get-nvrtc.sh --root DIR                       put .tools in DIR, not in the project's folder
#
# Only the GPU needs it. Without it, acc runs on the CPU alone, much more slowly. Run acc doctor afterwards to check.
set -eu

version=12.9.86
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
wheel=
sha256=

die() {
    echo "get-nvrtc.sh: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case $1 in
        --wheel | --sha256 | --root)
            [ $# -ge 2 ] || die "$1 needs a value"
            case $1 in
                --wheel) wheel=$2 ;;
                --sha256) sha256=$(printf '%s' "$2" | tr 'A-F' 'a-f') ;;
                --root) root=$2 ;;
            esac
            shift 2
            ;;
        -h | --help)
            sed -n '2,23p' "$0"
            exit 0
            ;;
        *) die "unknown option $1 (--help lists them)" ;;
    esac
done

[ -z "$sha256" ] || [ -n "$wheel" ] || die "--sha256 goes with --wheel: without it, the hash comes from PyPI"

python=
for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c 'import json, zipfile' >/dev/null 2>&1; then
        python=$candidate
        break
    fi
done
[ -n "$python" ] || die "this needs python3, to read PyPI's answer and to unpack the file; install it and run this again"

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        die "this needs curl or wget to download; install one and run this again"
    fi
}

sha() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        "$python" -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$1"
    fi
}

case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "PyPI has no NVRTC for this kind of CPU ($(uname -m))" ;;
esac

mkdir -p "$root"
root=$(cd "$root" && pwd)
tools=$root/.tools
dest=$tools/cuda-nvrtc-$version
if [ -e "$dest/nvidia/cuda_nvrtc/lib/libnvrtc.so" ]; then
    echo "NVRTC $version is there already, in $dest. Run acc doctor to check that acc finds it."
    exit 0
fi
[ ! -e "$dest" ] || die "$dest is there, but without nvidia/cuda_nvrtc/lib/libnvrtc.so in it; delete it and run this again"

# The file's name, address and SHA-256, from PyPI's listing of the version: the one named by --wheel, or else the
# Linux one for this CPU.
if [ -z "$sha256" ]; then
    listing=$(mktemp)
    trap 'rm -f "$listing"' EXIT
    echo "Asking PyPI for nvidia-cuda-nvrtc-cu12 $version..."
    fetch "https://pypi.org/pypi/nvidia-cuda-nvrtc-cu12/$version/json" "$listing" ||
        die "couldn't reach PyPI; check the internet connection and run this again"
    found=$("$python" - "$listing" "$arch" "$(basename "${wheel:-none}")" <<'EOF'
import json, sys
listing, arch, named = sys.argv[1:]
for u in json.load(open(listing))["urls"]:
    f = u["filename"]
    if f == named or (named == "none" and "manylinux" in f and f.endswith("_" + arch + ".whl")):
        print(f, u["url"], u["digests"]["sha256"])
        break
EOF
)
    [ -n "$found" ] || die "PyPI lists no such file of nvidia-cuda-nvrtc-cu12 $version (${wheel:-Linux, $arch}); with --wheel, give its SHA-256 with --sha256"
    set -- $found
    name=$1 url=$2 sha256=$3
fi

fetched=
if [ -z "$wheel" ]; then
    mkdir -p "$tools/downloads"
    wheel=$tools/downloads/$name
    # A copy from an earlier try is used if it is the right file.
    if [ ! -f "$wheel" ] || [ "$(sha "$wheel")" != "$sha256" ]; then
        echo "Downloading $url..."
        fetch "$url" "$wheel.partial" || die "the download didn't work; check the internet connection and run this again"
        mv "$wheel.partial" "$wheel"
    fi
    fetched=yes
fi

[ -f "$wheel" ] || die "there is no file $wheel"
actual=$(sha "$wheel")
[ "$actual" = "$sha256" ] || die "$wheel is not the file it should be: its SHA-256 is $actual, not $sha256. Nothing was unpacked."

# Unpacked beside its place first, so that a folder half unpacked is never taken for the real one.
partial=$dest.partial
rm -rf "$partial"
mkdir -p "$tools"
"$python" -m zipfile -e "$wheel" "$partial"
lib=$partial/nvidia/cuda_nvrtc/lib
real=$(cd "$lib" 2>/dev/null && ls libnvrtc.so.* 2>/dev/null | head -n 1) || true
[ -n "$real" ] || die "the file unpacked, but without nvidia/cuda_nvrtc/lib/libnvrtc.so.* in it; please report this"
[ -e "$lib/libnvrtc.so" ] || ln -s "$real" "$lib/libnvrtc.so"
mv "$partial" "$dest"

echo "NVRTC $version is in $dest"
echo "acc finds it there when it runs in $root, or when it is a copy under it, as target/release/acc is."
echo "Elsewhere, acc finds it when ACC_NVRTC_DIR names that folder: add this line to ~/.profile:"
echo "  export ACC_NVRTC_DIR=\"$dest\""
echo "Run acc doctor to check."
[ -z "$fetched" ] || echo "The downloaded file, $wheel, may be deleted now."
