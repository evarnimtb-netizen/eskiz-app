# Builds Eskiz for Windows x64 and packs it as a zip.
#
# Output: eskiz\dist\release\Eskiz-<version>-windows-x64.zip
#
# Zip layout (one folder, so it extracts cleanly):
#   Eskiz-<version>-windows-x64\
#     eskiz.exe     release binary; the icon and version info are embedded by build.rs
#     eskiz.ico     the app icon, for shortcuts
#     THIRD_PARTY_NOTICES.md, LICENSE-MIT, LICENSE-APACHE
#
# Run from anywhere on Windows with the MSVC Rust toolchain installed:
#   powershell -ExecutionPolicy Bypass -File eskiz\packaging\windows\build.ps1
# Pass -SkipBuild to package an existing target\x86_64-pc-windows-msvc\release\eskiz.exe.
[CmdletBinding()]
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$eskizDir = (Resolve-Path (Join-Path $here '..\..')).Path
$target = 'x86_64-pc-windows-msvc'
$outDir = Join-Path $eskizDir 'dist\release'

# The root Cargo.toml defines the version once, in [workspace.package].
$cargoToml = Get-Content -Raw -Path (Join-Path $eskizDir 'Cargo.toml')
$match = [regex]::Match($cargoToml, '(?m)^version = "([^"]+)"')
if (-not $match.Success) { throw "could not read the version from Cargo.toml" }
$version = $match.Groups[1].Value

Push-Location $eskizDir
try {
    if (-not $SkipBuild) {
        Write-Host "== cargo build ($target)"
        & rustup target add $target
        & cargo build --release --locked -p eskiz-app --target $target
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    }

    $exe = Join-Path $eskizDir "target\$target\release\eskiz.exe"
    if (-not (Test-Path $exe)) { throw "missing $exe (build first)" }

    Write-Host "== self-test"
    & $exe --selftest
    if ($LASTEXITCODE -ne 0) { throw "eskiz --selftest failed" }

    Write-Host "== package"
    $name = "Eskiz-$version-windows-x64"
    $stage = Join-Path $outDir $name
    $zip = Join-Path $outDir "$name.zip"
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null
    # Only this package's files are replaced; other outputs in dist\release are kept.
    Remove-Item -Recurse -Force -Path $stage -ErrorAction SilentlyContinue
    Remove-Item -Force -Path $zip -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item -Path $exe -Destination (Join-Path $stage 'eskiz.exe')
    Copy-Item -Path (Join-Path $eskizDir 'packaging\icons\eskiz.ico') -Destination (Join-Path $stage 'eskiz.ico')
    foreach ($f in 'THIRD_PARTY_NOTICES.md', 'LICENSE-MIT', 'LICENSE-APACHE') {
        $src = Join-Path $eskizDir $f
        if (-not (Test-Path $src)) { throw "missing $src" }
        Copy-Item -Path $src -Destination (Join-Path $stage $f)
    }
    Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal
    Remove-Item -Recurse -Force -Path $stage

    $size = [math]::Round((Get-Item $zip).Length / 1MB, 1)
    Write-Host "wrote $zip ($size MB)"
}
finally {
    Pop-Location
}
