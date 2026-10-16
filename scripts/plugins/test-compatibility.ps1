param(
    [Parameter(Mandatory = $true)][string]$ArtifactRoot
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$env:PHOTOCRAFT_PLUGIN_COMPAT_ROOT = (Resolve-Path -LiteralPath $ArtifactRoot).Path
$cacheRoot = Join-Path $repository 'target/plugin-compatibility-cache'
$cacheVariables = @('PHOTOCRAFT_PLUGIN_COMPAT_CACHE', 'LOCALAPPDATA', 'APPDATA', 'XDG_CACHE_HOME', 'TEMP', 'TMP')
$priorCacheEnvironment = @{}
foreach ($name in $cacheVariables) { $priorCacheEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$env:PHOTOCRAFT_PLUGIN_COMPAT_CACHE = $cacheRoot
$env:LOCALAPPDATA = Join-Path $cacheRoot 'Local'
$env:APPDATA = Join-Path $cacheRoot 'Roaming'
$env:XDG_CACHE_HOME = Join-Path $cacheRoot 'xdg'
$env:TEMP = Join-Path $cacheRoot 'Temp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force -Path $env:LOCALAPPDATA, $env:APPDATA, $env:XDG_CACHE_HOME, $env:TEMP | Out-Null
Push-Location -LiteralPath $repository
try {
    $catalog = Get-Content -LiteralPath (Join-Path $repository 'tests/plugin-compatibility/catalog.json') -Raw | ConvertFrom-Json
    foreach ($artifact in $catalog.artifacts) {
        $artifactPath = Join-Path $env:PHOTOCRAFT_PLUGIN_COMPAT_ROOT $artifact.name
        if (-not (Test-Path -LiteralPath $artifactPath -PathType Leaf)) {
            $artifactPath = Join-Path $env:PHOTOCRAFT_PLUGIN_COMPAT_ROOT $artifact.path
        }
        if (-not (Test-Path -LiteralPath $artifactPath -PathType Leaf)) {
            $artifactPath = Join-Path (Join-Path $env:PHOTOCRAFT_PLUGIN_COMPAT_ROOT 'upstream/photocraft') $artifact.path
        }
        if (-not (Test-Path -LiteralPath $artifactPath -PathType Leaf)) { throw "Missing immutable fixture: $($artifact.name)" }
        if ((Get-Item -LiteralPath $artifactPath).Length -ne $artifact.size) { throw "Fixture size changed: $($artifact.name)" }
        if ((Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $artifact.sha256) {
            throw "Native transport fixture SHA-256 changed: $($artifact.name). Restore the pinned executable bytes."
        }
    }
    & cargo test -p photocraft-engine --test plugin_compatibility --locked --offline
    if ($LASTEXITCODE -ne 0) { throw 'Portable compatibility fixtures failed.' }
    $cases = @(
        'pinned_native_packages_install_restart_cancel_and_repair',
        'pinned_image_gain_native_processes_and_applies'
    )
    foreach ($case in $cases) {
        & cargo test -p photocraft-engine --test plugin_compatibility --locked --offline $case -- --ignored --exact --nocapture
        if ($LASTEXITCODE -ne 0) { throw "Pinned artifact compatibility case failed: $case" }
    }
} finally {
    Pop-Location
    foreach ($name in $cacheVariables) { [Environment]::SetEnvironmentVariable($name, $priorCacheEnvironment[$name], 'Process') }
}
