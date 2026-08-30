param(
    [Parameter(Mandatory = $true)]
    [string]$PlatformTarball,
    [Parameter(Mandatory = $true)]
    [string]$MainTarball,
    [string]$AliasTarball,
    [string]$UpgradeFromVersion = "0.2.6"
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
$upgradeMainPrefix = Join-Path $workspace "upgrade-main-prefix"
$upgradeAliasPrefix = Join-Path $workspace "upgrade-alias-prefix"
$cache = Join-Path $workspace "cache"
$fixtures = Join-Path $workspace "fixtures"
$packs = Join-Path $workspace "packs"
New-Item -ItemType Directory -Force -Path $mainPrefix, $aliasPrefix, $upgradeMainPrefix, $upgradeAliasPrefix, $cache, $fixtures, $packs | Out-Null

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
        "FASTCTX_NPM_DRIVER",
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
        $actualVersion = (& $command --version | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) { throw "npm launcher --version failed" }
        $expectedVersion = "fastctx $($mainManifest.version)"
        if ($actualVersion -ne $expectedVersion) {
            throw "npm launcher version mismatch: expected $expectedVersion; got $actualVersion"
        }

        $lifecycleRoot = Join-Path $workspace ("lifecycle-" + [Guid]::NewGuid().ToString("N"))
        $lifecycleEnvironment = [ordered]@{
            "HOME" = (Join-Path $lifecycleRoot "home")
            "USERPROFILE" = (Join-Path $lifecycleRoot "home")
            "TMPDIR" = (Join-Path $lifecycleRoot "temp")
            "TMP" = (Join-Path $lifecycleRoot "temp")
            "TEMP" = (Join-Path $lifecycleRoot "temp")
            "LOCALAPPDATA" = (Join-Path $lifecycleRoot "local-app-data")
            "APPDATA" = (Join-Path $lifecycleRoot "app-data")
            "XDG_RUNTIME_DIR" = (Join-Path $lifecycleRoot "xdg-runtime")
            "XDG_CONFIG_HOME" = (Join-Path $lifecycleRoot "xdg-config")
            "XDG_CACHE_HOME" = (Join-Path $lifecycleRoot "xdg-cache")
            "XDG_DATA_HOME" = (Join-Path $lifecycleRoot "xdg-data")
            "FASTCTX_TEST_RUNTIME_IDLE_MS" = "1000"
        }
        $savedEnvironment = @{}
        foreach ($entry in $lifecycleEnvironment.GetEnumerator()) {
            if ($entry.Key -ne "FASTCTX_TEST_RUNTIME_IDLE_MS") {
                New-Item -ItemType Directory -Force -Path $entry.Value | Out-Null
            }
            $savedEnvironment[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key, "Process")
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, "Process")
        }
        if (-not $isWindowsPlatform) {
            chmod 700 $lifecycleEnvironment["XDG_RUNTIME_DIR"]
            if ($LASTEXITCODE -ne 0) { throw "cannot protect the isolated XDG runtime directory" }
        }
        $savedEnvironment["CODEX_HOME"] = [Environment]::GetEnvironmentVariable("CODEX_HOME", "Process")
        [Environment]::SetEnvironmentVariable("CODEX_HOME", $null, "Process")
        try {
            & node (Join-Path $PSScriptRoot "verify-launcher-lifecycle.js") $Launcher
            if ($LASTEXITCODE -ne 0) { throw "npm launcher lifecycle verification failed" }
        } finally {
            foreach ($entry in $savedEnvironment.GetEnumerator()) {
                [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, "Process")
            }
        }
    }

    function Assert-InstalledVersion([string]$InstallPrefix, [string]$Version) {
        $command = Get-InstalledCommand $InstallPrefix
        $actualVersion = (& $command --version | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) { throw "npm launcher --version failed" }
        if ($actualVersion -ne "fastctx $Version") {
            throw "npm launcher version mismatch: expected fastctx $Version; got $actualVersion"
        }
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

    # The released registry package is the independent upgrade oracle. A source-generated
    # "old" fixture would only prove the current packer against itself (2026-08-29).
    npm install --global --prefix $upgradeMainPrefix --ignore-scripts --include=optional --no-audit --no-fund --registry=https://registry.npmjs.org/ "fastctx@$UpgradeFromVersion"
    if ($LASTEXITCODE -ne 0) { throw "isolated fastctx@$UpgradeFromVersion baseline install failed" }
    Assert-InstalledVersion $upgradeMainPrefix $UpgradeFromVersion
    npm install --global --prefix $upgradeMainPrefix --ignore-scripts --offline --include=optional $localMainTarball
    if ($LASTEXITCODE -ne 0) { throw "isolated fastctx@$UpgradeFromVersion upgrade failed" }
    Assert-InstalledPackage $upgradeMainPrefix (Get-InstalledLauncher $upgradeMainPrefix "fastctx")

    if ($localAliasTarball) {
        npm install --global --prefix $upgradeAliasPrefix --ignore-scripts --include=optional --no-audit --no-fund --registry=https://registry.npmjs.org/ "codex-fastctx@$UpgradeFromVersion"
        if ($LASTEXITCODE -ne 0) { throw "isolated codex-fastctx@$UpgradeFromVersion baseline install failed" }
        Assert-InstalledVersion $upgradeAliasPrefix $UpgradeFromVersion
        npm install --global --prefix $upgradeAliasPrefix --ignore-scripts --offline --include=optional $localAliasTarball $platformTarball
        if ($LASTEXITCODE -ne 0) { throw "isolated codex-fastctx@$UpgradeFromVersion upgrade failed" }
        Assert-InstalledPackage $upgradeAliasPrefix (Get-InstalledLauncher $upgradeAliasPrefix "codex-fastctx")
    }
} finally {
    Remove-Item -LiteralPath $workspace -Recurse -Force -ErrorAction SilentlyContinue
}
