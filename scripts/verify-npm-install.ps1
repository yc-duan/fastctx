param(
    [Parameter(Mandatory = $true)]
    [string]$PlatformTarball,
    [Parameter(Mandatory = $true)]
    [string]$MainTarball,
    [string]$AliasTarball
)

$ErrorActionPreference = "Stop"
$utf8 = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8
$isWindowsPlatform = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
$tarCommand = "tar"
if ($isWindowsPlatform) {
    $windowsDirectory = [Environment]::GetFolderPath([Environment+SpecialFolder]::Windows)
    $nativeTar = Join-Path $windowsDirectory "System32/tar.exe"
    if (Test-Path -LiteralPath $nativeTar -PathType Leaf) {
        $tarCommand = $nativeTar
    }
}
$workspace = Join-Path ([System.IO.Path]::GetTempPath()) ("fastctx-npm-" + [Guid]::NewGuid().ToString("N"))
$mainPrefix = Join-Path $workspace "main-prefix"
$aliasPrefix = Join-Path $workspace "alias-prefix"
$cache = Join-Path $workspace "cache"
$fixtures = Join-Path $workspace "fixtures"
$packs = Join-Path $workspace "packs"
New-Item -ItemType Directory -Force -Path $mainPrefix, $aliasPrefix, $cache, $fixtures, $packs | Out-Null

function Expand-Package([string]$Tarball, [string]$Name) {
    $destination = Join-Path $fixtures $Name
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    & $tarCommand -xf $Tarball -C $destination
    if ($LASTEXITCODE -ne 0) { throw "cannot extract npm tarball $Tarball" }
    $package = Join-Path $destination "package"
    if (-not (Test-Path -LiteralPath (Join-Path $package "package.json") -PathType Leaf)) {
        throw "npm tarball $Tarball has no package/package.json"
    }
    $package
}

function Write-Manifest($Manifest, [string]$Path) {
    $json = $Manifest | ConvertTo-Json -Depth 20
    [System.IO.File]::WriteAllText($Path, $json + "`n", [System.Text.UTF8Encoding]::new($false))
}

function Pack-Fixture([string]$Directory) {
    Push-Location $Directory
    try {
        $json = (& npm pack --json --pack-destination $packs | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "npm pack failed for local dependency fixture $Directory" }
        $result = $json | ConvertFrom-Json
        Join-Path $packs $result[0].filename
    } finally {
        Pop-Location
    }
}

try {
    $env:npm_config_cache = $cache
    $platformTarball = (Resolve-Path -LiteralPath $PlatformTarball).Path
    $mainTarball = (Resolve-Path -LiteralPath $MainTarball).Path
    $aliasTarball = if ($AliasTarball) { (Resolve-Path -LiteralPath $AliasTarball).Path } else { $null }

    $platformDirectory = Expand-Package $platformTarball "platform"
    $platformManifest = Get-Content -LiteralPath (Join-Path $platformDirectory "package.json") -Raw | ConvertFrom-Json
    $mainDirectory = Expand-Package $mainTarball "main"
    $mainManifestPath = Join-Path $mainDirectory "package.json"
    $mainManifest = Get-Content -LiteralPath $mainManifestPath -Raw | ConvertFrom-Json
    $mainLauncher = Get-Content -LiteralPath (Join-Path $mainDirectory "launcher.js") -Raw
    foreach ($requiredMarker in @(
        "FASTCTX_NPM_LAUNCHER_VERSION",
        "FASTCTX_NPM_PACKAGE",
        "FASTCTX_NPM_MODE",
        "FASTCTX_NODE_EXECUTABLE",
        "FASTCTX_NPM_CLI",
        "FASTCTX_NPM_LAUNCHER",
        "FASTCTX_NPM_LAUNCHER_PID",
        "FASTCTX_NPM_HANDOFF",
        "UPDATE_HANDOFF_EXIT_CODE = 75"
    )) {
        if (-not $mainLauncher.Contains($requiredMarker)) {
            throw "main launcher is missing update provenance contract: $requiredMarker"
        }
    }
    $declaredPlatform = $mainManifest.optionalDependencies.PSObject.Properties[$platformManifest.name]
    if (-not $declaredPlatform -or $declaredPlatform.Value -ne $platformManifest.version -or $mainManifest.version -ne $platformManifest.version) {
        throw "main package does not declare the matching platform package as an optional dependency"
    }

    $localOptional = [ordered]@{}
    $localOptional[$platformManifest.name] = "file:" + ($platformTarball -replace '\\', '/')
    $mainManifest.optionalDependencies = $localOptional
    Write-Manifest $mainManifest $mainManifestPath
    $localMainTarball = Pack-Fixture $mainDirectory

    $localAliasTarball = $null
    if ($aliasTarball) {
        $aliasDirectory = Expand-Package $aliasTarball "alias"
        $aliasManifestPath = Join-Path $aliasDirectory "package.json"
        $aliasManifest = Get-Content -LiteralPath $aliasManifestPath -Raw | ConvertFrom-Json
        if ($aliasManifest.dependencies.fastctx -ne $mainManifest.version) {
            throw "alias package does not depend on the matching fastctx version"
        }
        if ($aliasManifest.bin.fastctx -ne "launcher.js") {
            throw "alias package does not expose the fastctx command through launcher.js"
        }
        $aliasLauncher = (Get-Content -LiteralPath (Join-Path $aliasDirectory "launcher.js") -Raw) -replace "`r`n", "`n"
        $expectedAliasLauncher = "#!/usr/bin/env node`n'use strict';`n`nprocess.env.FASTCTX_NPM_PACKAGE = 'codex-fastctx';`nprocess.env.FASTCTX_NPM_LAUNCHER = __filename;`nrequire('fastctx/launcher.js');`n"
        if ($aliasLauncher -ne $expectedAliasLauncher) {
            throw "alias launcher does not identify its package before forwarding"
        }
        $aliasManifest.dependencies.fastctx = "file:" + ($localMainTarball -replace '\\', '/')
        Write-Manifest $aliasManifest $aliasManifestPath
        $localAliasTarball = Pack-Fixture $aliasDirectory
    }

    function Get-InstalledCommand([string]$InstallPrefix) {
        if ($isWindowsPlatform) {
            return Join-Path $InstallPrefix "fastctx.cmd"
        }
        return Join-Path $InstallPrefix "bin/fastctx"
    }

    function Get-InstalledLauncher([string]$InstallPrefix, [string]$PackageName) {
        $modules = if ($isWindowsPlatform) {
            Join-Path $InstallPrefix "node_modules"
        } else {
            Join-Path $InstallPrefix "lib/node_modules"
        }
        $launcher = Join-Path $modules "$PackageName/launcher.js"
        if (-not (Test-Path -LiteralPath $launcher -PathType Leaf)) {
            throw "installed npm launcher is missing: $launcher"
        }
        $launcher
    }

    function Assert-InstalledPackage([string]$InstallPrefix, [string]$Launcher) {
        $command = Get-InstalledCommand $InstallPrefix
        & $command --version
        if ($LASTEXITCODE -ne 0) { throw "npm launcher --version failed" }

        & node (Join-Path $PSScriptRoot "verify-launcher-lifecycle.js") $Launcher
        if ($LASTEXITCODE -ne 0) { throw "npm launcher lifecycle verification failed" }
    }

    npm install --global --prefix $mainPrefix --ignore-scripts --offline --include=optional $localMainTarball
    if ($LASTEXITCODE -ne 0) { throw "isolated main-package npm install failed" }
    Assert-InstalledPackage $mainPrefix (Get-InstalledLauncher $mainPrefix "fastctx")

    if ($localAliasTarball) {
        # npm does not install a transitive optional file: tarball from an offline
        # packed fixture. Install the already-validated platform fixture at the
        # alias prefix so this leg tests the alias forwarding and lifecycle rather
        # than npm's local-file resolution quirk. Registry manifests were checked
        # above for the real published dependency graph.
        npm install --global --prefix $aliasPrefix --ignore-scripts --offline --include=optional $localAliasTarball $platformTarball
        if ($LASTEXITCODE -ne 0) { throw "isolated alias-package npm install failed" }
        Assert-InstalledPackage $aliasPrefix (Get-InstalledLauncher $aliasPrefix "codex-fastctx")
    }
} finally {
    Remove-Item -LiteralPath $workspace -Recurse -Force -ErrorAction SilentlyContinue
}
