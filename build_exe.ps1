param(
    [string]$Python = 'python'
)

$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    & $Python -m PyInstaller --version
    if ($LASTEXITCODE -ne 0) {
        throw '先に python -m pip install -r requirements-build.txt を実行してください。'
    }
    & $Python -m PyInstaller --noconfirm --clean PCB-Clearance.spec
    if ($LASTEXITCODE -ne 0) {
        throw 'exe のビルドに失敗しました。'
    }
    Write-Output (Join-Path $PSScriptRoot 'dist\PCB-Clearance.exe')
}
finally {
    Pop-Location
}
