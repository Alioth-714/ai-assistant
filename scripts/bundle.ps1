# 打包脚本（阶段 5C）
#
# 用途：构建 release 可执行文件，并在可用时产出安装包。
# 说明：
#   1. 前端是静态资产，`tauri.conf.json` 的 distDir 指向 ../frontend，
#      release 构建会把前端**嵌入** exe，所以改完前端必须重新打包。
#   2. 安装包（msi/nsis）需要 tauri-cli。本机未安装时脚本只产出 exe，
#      并把它复制到 dist/ 便于分发。
#
# 用法：
#   pwsh scripts/bundle.ps1              # 构建 + 尽力打包
#   pwsh scripts/bundle.ps1 -SkipBuild   # 直接用已有 release 产物打包

param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $root 'dist'
$exePath = Join-Path $root 'src-tauri\target\release\ai-assistant-app.exe'

Push-Location $root
try {
    if (-not $SkipBuild) {
        Write-Host '==> cargo build --release (src-tauri)' -ForegroundColor Cyan
        cargo build --release --manifest-path src-tauri/Cargo.toml
        if ($LASTEXITCODE -ne 0) { throw "release 构建失败（exit $LASTEXITCODE）" }
    }

    if (-not (Test-Path $exePath)) {
        throw "未找到 release 产物：$exePath（去掉 -SkipBuild 重试）"
    }

    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    $stamped = Join-Path $dist "ai-assistant-app.exe"
    Copy-Item $exePath $stamped -Force
    $sizeMb = [math]::Round((Get-Item $exePath).Length / 1MB, 1)
    Write-Host "==> 已复制 exe 到 dist\（${sizeMb}MB）" -ForegroundColor Green

    # tauri-cli 存在时顺带产出安装包
    $cli = Get-Command cargo-tauri -ErrorAction SilentlyContinue
    if ($cli) {
        Write-Host '==> 检测到 tauri-cli，开始打包安装包' -ForegroundColor Cyan
        cargo tauri build
        if ($LASTEXITCODE -ne 0) { Write-Warning "安装包构建失败（exit $LASTEXITCODE），exe 已可用" }
    }
    else {
        Write-Warning '未安装 tauri-cli，跳过安装包（msi/nsis）。'
        Write-Host '  需要安装包时执行：cargo install tauri-cli --version "^1"' -ForegroundColor Yellow
    }

    Write-Host ''
    Write-Host '产物：' -ForegroundColor Green
    Get-ChildItem $dist | ForEach-Object { Write-Host ("  " + $_.Name) }
}
finally {
    Pop-Location
}
