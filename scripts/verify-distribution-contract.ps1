$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$cargoVersion = (Select-String -LiteralPath (Join-Path $root "Cargo.toml") -Pattern '^version = "([^"]+)"$').Matches[0].Groups[1].Value
$author = "yc-duan <dy2958830371@gmail.com>"
$license = "MIT OR Apache-2.0"
$platforms = [ordered]@{
    "fastctx-win32-x64" = "@fastctx/win32-x64"
    "fastctx-linux-x64" = "@fastctx/linux-x64"
    "fastctx-darwin-x64" = "@fastctx/darwin-x64"
    "fastctx-darwin-arm64" = "@fastctx/darwin-arm64"
}

function Read-Manifest([string]$Directory) {
    Get-Content -LiteralPath (Join-Path $root "packages/$Directory/package.json") -Raw | ConvertFrom-Json
}

$main = Read-Manifest "fastctx"
$alias = Read-Manifest "codex-fastctx"
$allManifests = @($main, $alias)
foreach ($entry in $platforms.GetEnumerator()) {
    $manifest = Read-Manifest $entry.Key
    $allManifests += $manifest
    if ($manifest.name -ne $entry.Value) {
        throw "Platform package $($entry.Key) must be named $($entry.Value), got $($manifest.name)"
    }
}

foreach ($manifest in $allManifests) {
    if ($manifest.version -ne $cargoVersion) {
        throw "Package $($manifest.name) version $($manifest.version) does not match Cargo $cargoVersion"
    }
    if ($manifest.author -ne $author -or $manifest.license -ne $license) {
        throw "Package $($manifest.name) changed the release identity or license contract"
    }
    if ($manifest.publishConfig.PSObject.Properties.Name -contains "provenance") {
        throw "Package $($manifest.name) must not declare npm provenance for manual publishing"
    }
    if ($manifest.scripts -and ($manifest.scripts.install -or $manifest.scripts.postinstall)) {
        throw "Install scripts are forbidden in $($manifest.name)"
    }
}

$optionalNames = @($main.optionalDependencies.PSObject.Properties.Name | Sort-Object)
$expectedOptionalNames = @($platforms.Values | Sort-Object)
if ((Compare-Object -ReferenceObject $expectedOptionalNames -DifferenceObject $optionalNames).Count -ne 0) {
    throw "fastctx optionalDependencies must be exactly the four scoped platform packages"
}
foreach ($name in $expectedOptionalNames) {
    if ($main.optionalDependencies.$name -ne $cargoVersion) {
        throw "fastctx optional dependency $name must use version $cargoVersion"
    }
}
if ($alias.dependencies.fastctx -ne $cargoVersion) {
    throw "codex-fastctx must depend on fastctx@$cargoVersion"
}

$launcher = Get-Content -LiteralPath (Join-Path $root "packages/fastctx/launcher.js") -Raw
foreach ($name in $platforms.Values) {
    if (-not $launcher.Contains("'$name'")) {
        throw "npm launcher is missing scoped platform mapping $name"
    }
}

$tracked = @(& git -C $root ls-files)
if ($LASTEXITCODE -ne 0) {
    throw "Cannot inspect the public git tree"
}
$privatePaths = @(
    $tracked |
        Where-Object {
            $_ -match '^DESIGN(?:-[^/]+)?\.md$' -or
            $_ -match '(^|/)(AGENTS|CLAUDE)\.md$' -or
            $_ -match '^\.sisyphus/' -or
            $_ -match '^dev-notes/'
        }
)
if ($privatePaths.Count -ne 0) {
    throw "Private design or agent files leaked into the public tree: $($privatePaths -join ', ')"
}

foreach ($name in @("LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "THIRD_PARTY_LICENSES.md")) {
    if (-not (Test-Path -LiteralPath (Join-Path $root $name) -PathType Leaf)) {
        throw "Missing release license file: $name"
    }
}

$releaseWorkflow = Get-Content -LiteralPath (Join-Path $root ".github/workflows/release.yml") -Raw
foreach ($required in @(
    "finalize-release-assets.ps1",
    "verify-release-assets.ps1",
    "verify-release-identity.ps1 -TagName `$env:GITHUB_REF_NAME",
    "dist/release/*",
    "npm-tarballs-",
    "workflow_dispatch:",
    "cargo install cargo-zigbuild --locked --version 0.23.0",
    "cargo zigbuild --locked --release --target `${{ matrix.zig_target }}",
    "x86_64-unknown-linux-gnu.2.31",
    "Negative control correctly rejected",
    "libpdfium.so",
    "ubuntu:20.04",
    "ubuntu:22.04",
    "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/')"
)) {
    if (-not $releaseWorkflow.Contains($required)) {
        throw "Release workflow is missing distribution contract marker: $required"
    }
}
$releaseFinalizer = Get-Content -LiteralPath (Join-Path $root "scripts/finalize-release-assets.ps1") -Raw
if (-not $releaseFinalizer.Contains("SHA256SUMS")) {
    throw "Release finalizer must create the single SHA256SUMS asset"
}
if ($releaseWorkflow.Contains(".sha256")) {
    throw "Release workflow must not create per-asset .sha256 sidecars"
}
if ($releaseWorkflow -match 'gh release (?:create|upload)[^\r\n]*dist/\*') {
    throw "GitHub Release upload must be restricted to dist/release, not all workflow artifacts"
}

$identityVerifier = Join-Path $root "scripts/verify-release-identity.ps1"
$identityFixture = Join-Path ([System.IO.Path]::GetTempPath()) "fastctx-release-identity-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $identityFixture | Out-Null
try {
    Set-Content -LiteralPath (Join-Path $identityFixture "Cargo.toml") -Value @"
[package]
name = "release-identity-fixture"
version = "$cargoVersion"
"@
    Set-Content -LiteralPath (Join-Path $identityFixture "fixture.txt") -Value "release identity fixture"
    & git -C $identityFixture init --quiet
    if ($LASTEXITCODE -ne 0) { throw "Cannot initialize the release identity fixture" }
    & git -C $identityFixture config core.autocrlf false
    & git -C $identityFixture config user.name "FastCtx release identity fixture"
    & git -C $identityFixture config user.email "fixture@invalid.example"
    & git -C $identityFixture add Cargo.toml fixture.txt
    & git -C $identityFixture -c commit.gpgSign=false commit --quiet -m "release identity fixture"
    if ($LASTEXITCODE -ne 0) { throw "Cannot commit the release identity fixture" }
    $fixtureTag = "v$cargoVersion"
    & git -C $identityFixture -c tag.gpgSign=false tag -a $fixtureTag -m $fixtureTag
    if ($LASTEXITCODE -ne 0) { throw "Cannot create the annotated release identity fixture tag" }
    & $identityVerifier -RepositoryRoot $identityFixture -TagName $fixtureTag

    & git -C $identityFixture tag -d $fixtureTag | Out-Null
    & git -C $identityFixture tag $fixtureTag
    if ($LASTEXITCODE -ne 0) { throw "Cannot create the lightweight negative fixture tag" }
    $lightweightRejected = $false
    try {
        & $identityVerifier -RepositoryRoot $identityFixture -TagName $fixtureTag
    } catch {
        $lightweightRejected = $_.Exception.Message -eq "Release tag refs/tags/$fixtureTag must be an annotated tag"
    }
    if (-not $lightweightRejected) {
        throw "Release identity verification must reject a lightweight tag"
    }
} finally {
    Remove-Item -LiteralPath $identityFixture -Recurse -Force -ErrorAction SilentlyContinue
}

$forbiddenApiHost = "api" + ".github.com"
$networkContractFiles = @(
    (Join-Path $root "src"),
    (Join-Path $root "tests"),
    (Join-Path $root "scripts"),
    (Join-Path $root ".github"),
    (Join-Path $root "README.md"),
    (Join-Path $root "README.zh-CN.md")
)
foreach ($candidate in $networkContractFiles) {
    if (Test-Path -LiteralPath $candidate -PathType Container) {
        $matches = Get-ChildItem -LiteralPath $candidate -Recurse -File |
            Where-Object { $_.Extension -in @(".rs", ".ps1", ".yml", ".yaml", ".js", ".json", ".md") } |
            Select-String -SimpleMatch $forbiddenApiHost
    } else {
        $matches = Select-String -LiteralPath $candidate -SimpleMatch $forbiddenApiHost
    }
    if ($matches) {
        throw "The update path must never reference the rate-limited GitHub API host: $($matches.Path -join ', ')"
    }
}
