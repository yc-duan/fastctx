param(
    [Parameter(Mandatory = $true)]
    [string]$Target,
    [Parameter(Mandatory = $true)]
    [string]$Binary,
    [string]$OutputDirectory = "dist/release"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$mapping = @{
    "x86_64-pc-windows-msvc" = @{
        Archive = "fastctx-x86_64-pc-windows-msvc.zip"
        Binary = "fastctx.exe"
        Format = "zip"
    }
    "x86_64-unknown-linux-gnu" = @{
        Archive = "fastctx-x86_64-unknown-linux-gnu.tar.gz"
        Binary = "fastctx"
        Format = "tar.gz"
    }
    "x86_64-apple-darwin" = @{
        Archive = "fastctx-x86_64-apple-darwin.tar.gz"
        Binary = "fastctx"
        Format = "tar.gz"
    }
    "aarch64-apple-darwin" = @{
        Archive = "fastctx-aarch64-apple-darwin.tar.gz"
        Binary = "fastctx"
        Format = "tar.gz"
    }
}
$licenseFiles = @("LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD_PARTY_LICENSES.md")
$tarCommand = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) {
    Join-Path $env:SystemRoot "System32/tar.exe"
} else {
    "tar"
}

if (-not $mapping.ContainsKey($Target)) {
    throw "Unsupported release target: $Target"
}

$entry = $mapping[$Target]
$binaryPath = (Resolve-Path -LiteralPath $Binary -ErrorAction Stop).Path
$destination = Join-Path $root $OutputDirectory
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$archivePath = Join-Path $destination $entry.Archive
$stagingDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("fastctx-release-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $stagingDirectory | Out-Null

try {
    Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $stagingDirectory $entry.Binary)
    foreach ($name in $licenseFiles) {
        Copy-Item -LiteralPath (Join-Path $root $name) -Destination (Join-Path $stagingDirectory $name)
    }

    if ($entry.Format -eq "tar.gz") {
        chmod 755 (Join-Path $stagingDirectory $entry.Binary)
        if ($LASTEXITCODE -ne 0) {
            throw "Cannot set the executable bit on $($entry.Binary)"
        }
    }

    Remove-Item -LiteralPath $archivePath -Force -ErrorAction SilentlyContinue
    $archiveEntries = @($entry.Binary) + $licenseFiles
    if ($entry.Format -eq "zip") {
        Compress-Archive `
            -LiteralPath ($archiveEntries | ForEach-Object { Join-Path $stagingDirectory $_ }) `
            -DestinationPath $archivePath `
            -CompressionLevel Optimal
    } else {
        Push-Location (Split-Path -Parent $archivePath)
        try {
            & $tarCommand -czf (Split-Path -Leaf $archivePath) -C $stagingDirectory @archiveEntries
            if ($LASTEXITCODE -ne 0) {
                throw "Cannot create release archive $archivePath"
            }
        } finally {
            Pop-Location
        }
    }

    if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        throw "Release archive was not created: $archivePath"
    }
    Get-FileHash -LiteralPath $archivePath -Algorithm SHA256 | Format-List
    Write-Output $archivePath
} finally {
    Remove-Item -LiteralPath $stagingDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
