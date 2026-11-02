param(
    [string]$Cargo = 'cargo',
    [string]$Target = ''
)

$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    if (-not $Target -and $env:CARGO_BUILD_TARGET) {
        $Target = $env:CARGO_BUILD_TARGET
    }
    if ($Target) {
        & $Cargo build --release --locked --bins --target $Target
    }
    else {
        & $Cargo build --release --locked --bins
    }
    if ($LASTEXITCODE -ne 0) {
        throw 'exe のビルドに失敗しました。'
    }
    $targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot 'target' }
    if ($Target) {
        $targetRoot = Join-Path $targetRoot $Target
    }
    $distDir = Join-Path $PSScriptRoot 'dist'
    New-Item -ItemType Directory -Force -Path $distDir | Out-Null
    foreach ($binary in @(
        @{ Source = 'pcb-clearance-gui.exe'; Target = 'PCB-Clearance.exe' },
        @{ Source = 'pcb-clearance.exe'; Target = 'pcb-clearance-cli.exe' }
    )) {
        Copy-Item -LiteralPath (Join-Path $targetRoot "release\$($binary.Source)") -Destination (Join-Path $distDir $binary.Target) -Force
        Write-Output (Join-Path $distDir $binary.Target)
    }
}
finally {
    Pop-Location
}
