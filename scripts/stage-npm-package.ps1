param(
    [Parameter(Mandatory = $true)]
    [string]$Target,
    [Parameter(Mandatory = $true)]
    [string]$Binary,
    [string]$OutputDirectory = "dist/npm",
    [switch]$IncludeRootPackages
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$cargoVersion = (Select-String -LiteralPath (Join-Path $root "Cargo.toml") -Pattern '^version = "([^"]+)"$').Matches[0].Groups[1].Value

$mapping = @{
    "x86_64-pc-windows-msvc" = @{ Directory = "fastctx-win32-x64"; Package = "@fastctx/win32-x64"; Name = "fastctx.exe" }
    "x86_64-unknown-linux-gnu" = @{ Directory = "fastctx-linux-x64"; Package = "@fastctx/linux-x64"; Name = "fastctx" }
    "x86_64-apple-darwin" = @{ Directory = "fastctx-darwin-x64"; Package = "@fastctx/darwin-x64"; Name = "fastctx" }
    "aarch64-apple-darwin" = @{ Directory = "fastctx-darwin-arm64"; Package = "@fastctx/darwin-arm64"; Name = "fastctx" }
}

if (-not $mapping.ContainsKey($Target)) {
    throw "Unsupported npm target: $Target"
}

$entry = $mapping[$Target]
$packageDirectory = Join-Path $root "packages/$($entry.Directory)"
$binDirectory = Join-Path $packageDirectory "bin"

function Get-Sha256([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $hasher = [System.Security.Cryptography.SHA256]::Create()
        try {
            ([System.BitConverter]::ToString($hasher.ComputeHash($stream))).Replace("-", "")
        } finally {
            $hasher.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

New-Item -ItemType Directory -Force -Path $binDirectory | Out-Null
Copy-Item -LiteralPath $Binary -Destination (Join-Path $binDirectory $entry.Name) -Force
$isWindowsPlatform = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
if (-not $isWindowsPlatform) {
    chmod +x (Join-Path $binDirectory $entry.Name)
}
$sourceHash = Get-Sha256 $Binary
$stagedHash = Get-Sha256 (Join-Path $binDirectory $entry.Name)
if ($sourceHash -ne $stagedHash) {
    throw "Staged npm binary differs from the Release binary"
}

function Stage-Licenses([string]$Directory) {
    $licenseDirectory = Join-Path $Directory "licenses"
    New-Item -ItemType Directory -Force -Path $licenseDirectory | Out-Null
    foreach ($name in @("LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD_PARTY_LICENSES.md")) {
        Copy-Item -LiteralPath (Join-Path $root $name) -Destination (Join-Path $licenseDirectory $name) -Force
    }
}

Stage-Licenses $packageDirectory
$platformManifest = Get-Content -LiteralPath (Join-Path $packageDirectory "package.json") -Raw | ConvertFrom-Json
if ($platformManifest.name -ne $entry.Package) {
    throw "Platform package name mismatch for $Target`: $($platformManifest.name) != $($entry.Package)"
}

foreach ($manifest in Get-ChildItem -LiteralPath (Join-Path $root "packages") -Filter package.json -Recurse) {
    $package = Get-Content -LiteralPath $manifest.FullName -Raw | ConvertFrom-Json
    if ($package.version -ne $cargoVersion) {
        throw "Version mismatch in $($manifest.FullName): $($package.version) != $cargoVersion"
    }
    if ($package.scripts -and ($package.scripts.install -or $package.scripts.postinstall)) {
        throw "Install scripts are forbidden in $($manifest.FullName)"
    }
}

$destination = Join-Path $root $OutputDirectory
New-Item -ItemType Directory -Force -Path $destination | Out-Null

function Pack-CheckedPackage([string]$Directory, [string[]]$AllowedFiles) {
    Push-Location $Directory
    try {
        $json = (& npm pack --json --pack-destination $destination | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "npm pack failed for $Directory" }
        $result = $json | ConvertFrom-Json
        $actual = @($result[0].files | ForEach-Object { $_.path })
        foreach ($path in $actual) {
            if ($path -notin $AllowedFiles) {
                throw "Unexpected file in npm tarball for $Directory`: $path"
            }
        }
        foreach ($path in $AllowedFiles) {
            if ($path -notin $actual) {
                throw "Missing file in npm tarball for $Directory`: $path"
            }
        }
        $result[0]
    } finally {
        Pop-Location
    }
}

$licenseFiles = @("licenses/LICENSE-MIT", "licenses/LICENSE-APACHE", "licenses/NOTICE", "licenses/THIRD_PARTY_LICENSES.md")
Pack-CheckedPackage $packageDirectory (@("package.json", "bin/$($entry.Name)") + $licenseFiles)

if ($IncludeRootPackages) {
    Stage-Licenses (Join-Path $root "packages/fastctx")
    Stage-Licenses (Join-Path $root "packages/codex-fastctx")
    Pack-CheckedPackage (Join-Path $root "packages/fastctx") (@("package.json", "launcher.js", "README.md") + $licenseFiles)
    Pack-CheckedPackage (Join-Path $root "packages/codex-fastctx") (@("package.json", "launcher.js", "README.md") + $licenseFiles)
}
