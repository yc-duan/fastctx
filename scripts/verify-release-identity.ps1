param(
    [Parameter(Mandatory = $true)]
    [string]$TagName,
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = "Stop"

$cargoPath = Join-Path $RepositoryRoot "Cargo.toml"
$versionMatches = @(
    Select-String -LiteralPath $cargoPath -Pattern '^version = "([^"]+)"$'
)
if ($versionMatches.Count -ne 1 -or $versionMatches[0].Matches.Count -ne 1) {
    throw "Cargo.toml must contain exactly one package version"
}
$cargoVersion = $versionMatches[0].Matches[0].Groups[1].Value
$expectedTag = "v$cargoVersion"
if ($TagName -ne $expectedTag) {
    throw "Release tag $TagName does not match Cargo version $cargoVersion (expected $expectedTag)"
}

$tagRef = "refs/tags/$TagName"
$tagType = (& git -C $RepositoryRoot cat-file -t $tagRef 2>$null)
if ($LASTEXITCODE -ne 0) {
    throw "Release tag $tagRef does not exist in the checked-out repository"
}
if (($tagType | Out-String).Trim() -ne "tag") {
    throw "Release tag $tagRef must be an annotated tag"
}

$tagCommit = (& git -C $RepositoryRoot rev-parse "$tagRef^{commit}" 2>$null)
if ($LASTEXITCODE -ne 0) {
    throw "Cannot resolve release tag $tagRef to a commit"
}
$headCommit = (& git -C $RepositoryRoot rev-parse "HEAD" 2>$null)
if ($LASTEXITCODE -ne 0) {
    throw "Cannot resolve the checked-out release commit"
}
if (($tagCommit | Out-String).Trim() -ne ($headCommit | Out-String).Trim()) {
    throw "Release tag $tagRef must point at the checked-out commit"
}
