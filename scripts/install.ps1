# Usage: irm https://raw.githubusercontent.com/methamphetaminelab/gramtty/main/scripts/install.ps1 | iex
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = 'methamphetaminelab/gramtty'
$installDir = if ($env:GRAMTTY_INSTALL_DIR) { $env:GRAMTTY_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\gramtty' }

if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'gramtty needs 64-bit Windows'
}

if ($env:GRAMTTY_VERSION) {
    $version = $env:GRAMTTY_VERSION.TrimStart('v')
} else {
    $version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name.TrimStart('v')
}

$name = "gramtty-$version-x86_64-windows"
$asset = "$name.zip"
$base = "https://github.com/$repo/releases/download/v$version"
$tmp = Join-Path ([IO.Path]::GetTempPath()) "gramtty-install-$([Guid]::NewGuid())"
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Downloading $asset"
    Invoke-WebRequest "$base/$asset" -OutFile "$tmp\$asset" -UseBasicParsing
    Invoke-WebRequest "$base/$asset.sha256" -OutFile "$tmp\$asset.sha256" -UseBasicParsing
    $expected = ((Get-Content "$tmp\$asset.sha256" -Raw).Trim() -split '\s+')[0].ToLower()
    $actual = (Get-FileHash "$tmp\$asset" -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) {
        throw "checksum mismatch for $asset (expected $expected, got $actual)"
    }
    Write-Host 'Checksum OK'
    Expand-Archive "$tmp\$asset" -DestinationPath $tmp
    $exe = Join-Path $tmp "$name\gramtty.exe"
    if (-not (Test-Path $exe)) {
        throw "$asset does not contain $name\gramtty.exe"
    }
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item $exe (Join-Path $installDir 'gramtty.exe') -Force
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "Installed gramtty $version to $installDir\gramtty.exe"

$parts = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ })
if ($parts -notcontains $installDir) {
    [Environment]::SetEnvironmentVariable('Path', (($parts + $installDir) -join ';'), 'User')
    $env:Path = "$env:Path;$installDir"
    Write-Host "Added $installDir to PATH"
}

if ($env:GRAMTTY_NO_DEPS -eq '1') {
    Write-Host 'Skipping mpv, ffmpeg and Windows Terminal (GRAMTTY_NO_DEPS=1)'
} else {
    Invoke-Expression (Invoke-RestMethod "https://raw.githubusercontent.com/$repo/v$version/scripts/install-deps.ps1")
}
