param(
    [string]$ReleaseDirectory = "dist/release",
    [string]$NpmDirectory = "dist/npm"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$releasePath = Join-Path $root $ReleaseDirectory
$npmPath = Join-Path $root $NpmDirectory
$version = (Select-String -LiteralPath (Join-Path $root "Cargo.toml") -Pattern '^version = "([^"]+)"$').Matches[0].Groups[1].Value
$archives = [ordered]@{
    "fastctx-x86_64-pc-windows-msvc.zip" = "fastctx.exe"
    "fastctx-x86_64-unknown-linux-gnu.tar.gz" = "fastctx"
    "fastctx-x86_64-apple-darwin.tar.gz" = "fastctx"
    "fastctx-aarch64-apple-darwin.tar.gz" = "fastctx"
}
$releaseFiles = @($archives.Keys) + @("SHA256SUMS")
$licenseFiles = @("LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD_PARTY_LICENSES.md")
$tarCommand = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) {
    Join-Path $env:SystemRoot "System32/tar.exe"
} else {
    "tar"
}

function Assert-ExactFiles([string]$Directory, [string[]]$Expected, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Directory -PathType Container)) {
        throw "$Label directory does not exist: $Directory"
    }
    $actual = @(
        Get-ChildItem -LiteralPath $Directory -File |
            ForEach-Object { $_.Name } |
            Sort-Object
    )
    $sortedExpected = @($Expected | Sort-Object)
    if ((Compare-Object -ReferenceObject $sortedExpected -DifferenceObject $actual).Count -ne 0) {
        throw "$Label file set mismatch. Expected [$($sortedExpected -join ', ')]; got [$($actual -join ', ')]."
    }
}

Assert-ExactFiles $releasePath $releaseFiles "Release"

$checksumLines = Get-Content -LiteralPath (Join-Path $releasePath "SHA256SUMS")
$checksums = @{}
foreach ($line in $checksumLines) {
    if ($line -notmatch '^([0-9a-f]{64})  ([^/\\]+)$') {
        throw "Invalid SHA256SUMS line: $line"
    }
    if ($checksums.ContainsKey($Matches[2])) {
        throw "Duplicate SHA256SUMS entry: $($Matches[2])"
    }
    $checksums[$Matches[2]] = $Matches[1]
}
if ((Compare-Object -ReferenceObject @($archives.Keys | Sort-Object) -DifferenceObject @($checksums.Keys | Sort-Object)).Count -ne 0) {
    throw "SHA256SUMS does not cover exactly the four release archives"
}
foreach ($name in $archives.Keys) {
    $actualHash = (Get-FileHash -LiteralPath (Join-Path $releasePath $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($checksums[$name] -ne $actualHash) {
        throw "SHA-256 mismatch for $name"
    }
}

$workspace = Join-Path ([System.IO.Path]::GetTempPath()) ("fastctx-release-verify-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $workspace | Out-Null
try {
    foreach ($archive in $archives.GetEnumerator()) {
        $destination = Join-Path $workspace ([System.IO.Path]::GetFileNameWithoutExtension($archive.Key))
        New-Item -ItemType Directory -Path $destination | Out-Null
        $assetPath = Join-Path $releasePath $archive.Key
        if ($archive.Key.EndsWith(".zip")) {
            Expand-Archive -LiteralPath $assetPath -DestinationPath $destination
        } else {
            Push-Location $releasePath
            try {
                & $tarCommand -xzf $archive.Key -C $destination
                if ($LASTEXITCODE -ne 0) {
                    throw "Cannot extract release archive $($archive.Key)"
                }
            } finally {
                Pop-Location
            }
        }
        $expectedContents = @($archive.Value) + $licenseFiles
        $actualContents = @(
            Get-ChildItem -LiteralPath $destination -Recurse -File |
                ForEach-Object { [System.IO.Path]::GetRelativePath($destination, $_.FullName).Replace("\", "/") } |
                Sort-Object
        )
        if ((Compare-Object -ReferenceObject @($expectedContents | Sort-Object) -DifferenceObject $actualContents).Count -ne 0) {
            throw "Archive content mismatch for $($archive.Key): [$($actualContents -join ', ')]"
        }
        $directories = @(Get-ChildItem -LiteralPath $destination -Recurse -Directory)
        if ($directories.Count -ne 0) {
            throw "Release archive $($archive.Key) contains a directory; contents must be flat"
        }
        if (-not $archive.Key.EndsWith(".zip") -and -not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)) {
            & /usr/bin/test -x (Join-Path $destination $archive.Value)
            if ($LASTEXITCODE -ne 0) {
                throw "Unix executable bit was not preserved in $($archive.Key)"
            }
        }
    }
} finally {
    Remove-Item -LiteralPath $workspace -Recurse -Force -ErrorAction SilentlyContinue
}

$npmPackages = [ordered]@{
    "fastctx-win32-x64-$version.tgz" = "@fastctx/win32-x64"
    "fastctx-linux-x64-$version.tgz" = "@fastctx/linux-x64"
    "fastctx-darwin-x64-$version.tgz" = "@fastctx/darwin-x64"
    "fastctx-darwin-arm64-$version.tgz" = "@fastctx/darwin-arm64"
    "fastctx-$version.tgz" = "fastctx"
    "codex-fastctx-$version.tgz" = "codex-fastctx"
}
Assert-ExactFiles $npmPath @($npmPackages.Keys) "npm workflow artifact"
foreach ($package in $npmPackages.GetEnumerator()) {
    Push-Location $npmPath
    try {
        $manifestJson = (& $tarCommand -xOf $package.Key "package/package.json" | Out-String)
        if ($LASTEXITCODE -ne 0) {
            throw "Cannot read package.json from $($package.Key)"
        }
    } finally {
        Pop-Location
    }
    $manifest = $manifestJson | ConvertFrom-Json
    if ($manifest.name -ne $package.Value -or $manifest.version -ne $version) {
        throw "npm tarball identity mismatch for $($package.Key): $($manifest.name)@$($manifest.version)"
    }
}
