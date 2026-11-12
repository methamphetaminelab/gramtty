# Usage: irm https://raw.githubusercontent.com/methamphetaminelab/gramtty/main/scripts/install-deps.ps1 | iex
$ErrorActionPreference = 'Stop'

function Add-UserPath($dir) {
    $parts = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ })
    if ($parts -notcontains $dir) {
        [Environment]::SetEnvironmentVariable('Path', (($parts + $dir) -join ';'), 'User')
    }
    if (($env:Path -split ';') -notcontains $dir) {
        $env:Path = "$env:Path;$dir"
    }
}

function Install-Package($id, $command) {
    if (Get-Command $command -ErrorAction SilentlyContinue) {
        Write-Host "$command is already installed"
        return
    }
    Write-Host "Installing $id"
    winget install --exact --id $id --source winget --accept-source-agreements --accept-package-agreements
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "winget could not install $id (exit code $LASTEXITCODE)"
    }
}

if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    Write-Warning 'winget is not available; install "App Installer" from Microsoft Store, or put mpv and ffmpeg on PATH yourself'
    return
}

Install-Package 'Gyan.FFmpeg' 'ffmpeg'
Install-Package 'shinchiro.mpv' 'mpv'
$mpvDir = Join-Path $env:ProgramFiles 'MPV Player'
if (-not (Get-Command mpv -ErrorAction SilentlyContinue) -and (Test-Path (Join-Path $mpvDir 'mpv.exe'))) {
    Add-UserPath $mpvDir
}
$terminal = $null
try {
    $terminal = Get-AppxPackage -Name Microsoft.WindowsTerminal | Sort-Object { [version]$_.Version } -Descending | Select-Object -First 1
} catch {
    $terminal = $null
}
if ($terminal -and [version]$terminal.Version -ge [version]'1.22') {
    Write-Host "Windows Terminal $($terminal.Version) is installed"
} elseif (-not $terminal -and (Get-Command wt -ErrorAction SilentlyContinue)) {
    Write-Host 'Windows Terminal is installed'
} else {
    Write-Host 'Installing Windows Terminal (1.22 or newer shows pictures)'
    winget install --exact --id Microsoft.WindowsTerminal --source winget --accept-source-agreements --accept-package-agreements
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "winget could not install Microsoft.WindowsTerminal (exit code $LASTEXITCODE)"
    }
}
Write-Host 'Dependencies are ready. Open a new terminal window so PATH changes apply.'
