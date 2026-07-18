param(
    [string]$OutputDirectory = "dist/npm"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$destination = Join-Path $root $OutputDirectory
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$cargoVersion = (Select-String -LiteralPath (Join-Path $root "Cargo.toml") -Pattern '^version = "([^"]+)"$').Matches[0].Groups[1].Value
$licenseFiles = @("licenses/LICENSE-MIT", "licenses/LICENSE-APACHE", "licenses/NOTICE", "licenses/THIRD_PARTY_LICENSES.md")

function Stage-Licenses([string]$Directory) {
    $licenseDirectory = Join-Path $Directory "licenses"
    New-Item -ItemType Directory -Force -Path $licenseDirectory | Out-Null
    foreach ($name in @("LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD_PARTY_LICENSES.md")) {
        Copy-Item -LiteralPath (Join-Path $root $name) -Destination (Join-Path $licenseDirectory $name) -Force
    }
}

function Pack-CheckedPackage([string]$Name, [string[]]$AllowedFiles) {
    $directory = Join-Path $root "packages/$Name"
    Stage-Licenses $directory
    $manifest = Get-Content -LiteralPath (Join-Path $directory "package.json") -Raw | ConvertFrom-Json
    if ($manifest.version -ne $cargoVersion) {
        throw "Version mismatch in $Name`: $($manifest.version) != $cargoVersion"
    }
    if ($manifest.scripts -and ($manifest.scripts.install -or $manifest.scripts.postinstall)) {
        throw "Install scripts are forbidden in $Name"
    }
    Push-Location $directory
    try {
        $json = (& npm pack --json --pack-destination $destination | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "npm pack failed for $Name" }
        $result = $json | ConvertFrom-Json
        $actual = @($result[0].files | ForEach-Object { $_.path })
        foreach ($path in $actual) {
            if ($path -notin $AllowedFiles) { throw "Unexpected file in $Name tarball: $path" }
        }
        foreach ($path in $AllowedFiles) {
            if ($path -notin $actual) { throw "Missing file in $Name tarball: $path" }
        }
        $result[0]
    } finally {
        Pop-Location
    }
}

Pack-CheckedPackage "fastctx" (@("package.json", "launcher.js", "README.md") + $licenseFiles)
Pack-CheckedPackage "codex-fastctx" (@("package.json", "launcher.js", "README.md") + $licenseFiles)
