param(
    [string]$ReleaseDirectory = "dist/release"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$directory = Join-Path $root $ReleaseDirectory
$expectedArchives = @(
    "fastctx-x86_64-pc-windows-msvc.zip",
    "fastctx-x86_64-unknown-linux-gnu.tar.gz",
    "fastctx-x86_64-apple-darwin.tar.gz",
    "fastctx-aarch64-apple-darwin.tar.gz"
)

if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
    throw "Release directory does not exist: $directory"
}

$actual = @(
    Get-ChildItem -LiteralPath $directory -File |
        Where-Object { $_.Name -ne "SHA256SUMS" } |
        ForEach-Object { $_.Name } |
        Sort-Object
)
$expected = @($expectedArchives | Sort-Object)
if ((Compare-Object -ReferenceObject $expected -DifferenceObject $actual).Count -ne 0) {
    throw "Release archive set mismatch. Expected [$($expected -join ', ')]; got [$($actual -join ', ')]."
}

$lines = foreach ($name in $expectedArchives) {
    $hash = (Get-FileHash -LiteralPath (Join-Path $directory $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $name"
}
$content = ($lines -join "`n") + "`n"
[System.IO.File]::WriteAllText(
    (Join-Path $directory "SHA256SUMS"),
    $content,
    [System.Text.UTF8Encoding]::new($false)
)
