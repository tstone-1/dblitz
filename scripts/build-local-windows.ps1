# Build the local portable exe and verify its embedded version, retaining Cargo's dependency cache.
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$target = Join-Path $repo 'src-tauri/target'
$previousTarget = $env:CARGO_TARGET_DIR
$exe = Join-Path $target 'release/dblitz.exe'
$backup = "$exe.$([guid]::NewGuid().ToString('N')).previous"
$ok = $false
Push-Location $repo
try {
    $version = (Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
    if (Test-Path -LiteralPath $exe) { Move-Item -LiteralPath $exe -Destination $backup }
    $env:CARGO_TARGET_DIR = $target
    # A portable executable requires no installer or updater signing operation.
    & (Join-Path $repo 'node_modules/.bin/tauri.cmd') build --no-bundle
    if ($LASTEXITCODE -ne 0) { throw "Tauri build failed (exit $LASTEXITCODE)" }
    $actual = (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion
    if ($actual -ne $version) { throw "Expected dblitz $version, built '$actual'" }
    $ok = $true
    Write-Output "[OK] Built dblitz $version at $exe"
} finally {
    if (Test-Path -LiteralPath $backup) {
        if ($ok) { Remove-Item -LiteralPath $backup }
        else { Move-Item -LiteralPath $backup -Destination $exe -Force }
    }
    $env:CARGO_TARGET_DIR = $previousTarget
    Pop-Location
}
