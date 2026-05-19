Write-Host "===========================================" -ForegroundColor Cyan
Write-Host "    MC Link Client Build Script" -ForegroundColor Cyan
Write-Host "===========================================" -ForegroundColor Cyan

$version = (Get-Content "package.json" -Raw | ConvertFrom-Json -ErrorAction Stop).version
Write-Host "`nBuilding version: $version" -ForegroundColor Yellow

Write-Host "`nBuilding frontend..." -ForegroundColor Green
pnpm build
if ($LASTEXITCODE -ne 0) {
    Write-Host "Frontend build failed!" -ForegroundColor Red
    Exit 1
}
Write-Host "Frontend built successfully" -ForegroundColor Green

Write-Host "`nBuilding Tauri client..." -ForegroundColor Green
pnpm tauri build
if ($LASTEXITCODE -ne 0) {
    Write-Host "Tauri build failed!" -ForegroundColor Red
    Exit 1
}
Write-Host "Tauri client built successfully" -ForegroundColor Green

Write-Host