<#
.SYNOPSIS
Fetches NVRTC, the free compiler acc needs to use an NVIDIA card, and puts it where acc looks for it.

.DESCRIPTION
NVRTC is NVIDIA's CUDA runtime compiler: acc compiles its GPU code with it each time a run starts. It is NVIDIA's
software, under NVIDIA's licence (the CUDA Toolkit end user licence agreement, which comes with it, in
nvidia_cuda_nvrtc_cu12-12.9.86.dist-info\licenses\License.txt). It isn't part of this project and isn't in its
repository. NVIDIA publishes it on PyPI, the Python package index, as the package nvidia-cuda-nvrtc-cu12.

This script downloads version 12.9.86 for 64-bit Windows from there (a 76 MB file, kept in .tools\downloads),
checks that it is exactly the file it should be (by its SHA-256), and unpacks it into .tools\cuda-nvrtc-12.9.86 in
the project's folder. Python is not needed: the file is a zip archive. Nothing else on the computer is changed.

Only the GPU needs it. Without it, acc runs on the CPU alone, much more slowly. Run acc doctor afterwards to check.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File tools\get-nvrtc.ps1

.EXAMPLE
# A copy of the file you downloaded yourself: it is checked in the same way, then unpacked.
powershell -ExecutionPolicy Bypass -File tools\get-nvrtc.ps1 -Wheel $HOME\Downloads\nvidia_cuda_nvrtc_cu12-12.9.86-py3-none-win_amd64.whl
#>
param(
    # The file, if you have downloaded it already: then nothing is downloaded.
    [string]$Wheel,
    # The folder to put .tools in: by default the project's folder, the one above this script's.
    [string]$Root
)

$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
# A full path: .NET, which unpacks the file, has a current folder of its own.
$Root = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Root)
$version = '12.9.86'
$name = "nvidia_cuda_nvrtc_cu12-$version-py3-none-win_amd64.whl"
$sha256 = '72972ebdcf504d69462d3bcd67e7b81edd25d0fb85a2c46d3ea3517666636349'
$tools = Join-Path $Root '.tools'
$dest = Join-Path $tools "cuda-nvrtc-$version"
$dll = Join-Path $dest 'nvidia\cuda_nvrtc\bin\nvrtc64_120_0.dll'

if (Test-Path $dll) {
    Write-Host "NVRTC $version is there already, in $dest. Run acc doctor to check that acc finds it."
    exit 0
}
if (Test-Path $dest) {
    Write-Host "$dest is there, but without nvrtc64_120_0.dll in it. Delete that folder and run this again."
    exit 1
}

$fetched = -not $Wheel
if ($fetched) {
    $downloads = Join-Path $tools 'downloads'
    $Wheel = Join-Path $downloads $name
    # A copy from an earlier try is used if it is the right file.
    if (-not (Test-Path -LiteralPath $Wheel) -or (Get-FileHash -Algorithm SHA256 -LiteralPath $Wheel).Hash -ne $sha256) {
        try {
            New-Item -ItemType Directory -Force $downloads | Out-Null
            # Windows PowerShell 5.1 may not offer TLS 1.2, which PyPI needs, unless asked.
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            Write-Host "Asking PyPI where $name is..."
            $listing = Invoke-RestMethod -UseBasicParsing "https://pypi.org/pypi/nvidia-cuda-nvrtc-cu12/$version/json"
            $entry = @($listing.urls | Where-Object { $_.filename -eq $name })[0]
            if (-not $entry) {
                Write-Host "PyPI doesn't list $name any more. docs/GPU_SETUP.md has other ways to get NVRTC."
                exit 1
            }
            Write-Host "Downloading $($entry.url) (76 MB)..."
            # The progress bar makes Windows PowerShell download many times more slowly.
            $ProgressPreference = 'SilentlyContinue'
            Invoke-WebRequest -UseBasicParsing -Uri $entry.url -OutFile "$Wheel.partial"
            Move-Item -Force "$Wheel.partial" $Wheel
        } catch {
            Write-Host "The download didn't work: $_"
            Write-Host 'Check the internet connection and run this again. Nothing was unpacked.'
            exit 1
        }
    }
}

if (-not (Test-Path -LiteralPath $Wheel -PathType Leaf)) {
    Write-Host "There is no file $Wheel."
    exit 1
}
$Wheel = (Resolve-Path -LiteralPath $Wheel).Path
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Wheel).Hash
if ($actual -ne $sha256) {
    Write-Host "$Wheel is not the file it should be: its SHA-256 is $actual, not $sha256."
    Write-Host "Nothing was unpacked. The right file is $name from PyPI."
    exit 1
}

# Unpacked beside its place first, so that a folder half unpacked is never taken for the real one.
$partial = "$dest.partial"
if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
New-Item -ItemType Directory -Force $tools | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::ExtractToDirectory($Wheel, $partial)
if (-not (Test-Path (Join-Path $partial 'nvidia\cuda_nvrtc\bin\nvrtc64_120_0.dll'))) {
    Write-Host "The file unpacked, but without nvidia\cuda_nvrtc\bin\nvrtc64_120_0.dll in it. Please report this."
    exit 1
}
Rename-Item -LiteralPath $partial (Split-Path -Leaf $dest)

Write-Host "NVRTC $version is in $dest"
Write-Host "acc finds it there when it runs in $Root, or when it is a copy under it, as target\release\acc.exe is."
Write-Host 'Elsewhere, acc finds it when the environment variable ACC_NVRTC_DIR names that folder. To set it for windows'
Write-Host "opened from now on: setx ACC_NVRTC_DIR `"$dest`""
Write-Host 'Run acc doctor to check.'
if ($fetched) { Write-Host "The downloaded file, $Wheel, may be deleted now." }
