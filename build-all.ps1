Write-Host "===========================================" -ForegroundColor Cyan
Write-Host "      MC Link Build Script" -ForegroundColor Cyan
Write-Host "===========================================" -ForegroundColor Cyan

$version = node -e "console.log(require('./package.json').version)"
Write-Host "`nVersion: $version" -ForegroundColor Yellow

Write-Host "`n[1/3] Building Central Server