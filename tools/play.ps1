<#
Starts Elden Ring with Seamless Co-op, making sure Steam is up first.

Seamless Co-op quits the game a few seconds after launch, with no message, when Steam isn't
running or hasn't finished logging in. This starts Steam if needed, waits until it's logged in,
then runs ersc_launcher.exe. Double-click play.bat in the repo root, or run:

    powershell -ExecutionPolicy Bypass -File tools\play.ps1 [-Game "<...\ELDEN RING\Game>"] [-DryRun]

-Game    The game folder, if the game isn't in one of this PC's Steam libraries.
-DryRun  Say what would happen; start nothing.
#>
param(
    [string]$Game,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'

function Find-SteamExe {
    $reg = Get-ItemProperty 'HKCU:\Software\Valve\Steam' -ErrorAction SilentlyContinue
    $candidates = @()
    if ($reg -and $reg.SteamExe) { $candidates += [string]$reg.SteamExe }
    if ($reg -and $reg.SteamPath) { $candidates += (Join-Path ([string]$reg.SteamPath) 'steam.exe') }
    $candidates += 'C:\Program Files (x86)\Steam\steam.exe'
    foreach ($exe in $candidates) {
        if (Test-Path -LiteralPath $exe) { return (Resolve-Path -LiteralPath $exe).Path }
    }
    throw "Can't find steam.exe. Is Steam installed?"
}

function Find-GameDir([string]$steamExe) {
    if ($Game) {
        if (Test-Path -LiteralPath (Join-Path $Game 'ersc_launcher.exe')) { return $Game }
        throw "No ersc_launcher.exe in $Game. Is Seamless Co-op installed there?"
    }
    # Every Steam library on this PC: the install folder plus those in libraryfolders.vdf.
    $steamDir = Split-Path -Parent $steamExe
    $libraries = @($steamDir)
    $vdf = Join-Path $steamDir 'steamapps\libraryfolders.vdf'
    if (Test-Path -LiteralPath $vdf) {
        foreach ($m in [regex]::Matches((Get-Content -LiteralPath $vdf -Raw), '"path"\s+"([^"]+)"')) {
            $libraries += ($m.Groups[1].Value -replace '\\\\', '\')
        }
    }
    foreach ($library in $libraries) {
        $dir = Join-Path $library 'steamapps\common\ELDEN RING\Game'
        if (Test-Path -LiteralPath (Join-Path $dir 'ersc_launcher.exe')) { return $dir }
    }
    throw "Can't find ELDEN RING\Game with ersc_launcher.exe in any Steam library. Pass -Game <folder>."
}

function Test-SteamLoggedIn {
    # Steam records its process id and the logged-in account under ActiveProcess. The values stay
    # behind after it exits, so they only count while the process id is the running Steam's.
    $steam = Get-Process -Name steam -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $steam) { return $false }
    $active = Get-ItemProperty 'HKCU:\Software\Valve\Steam\ActiveProcess' -ErrorAction SilentlyContinue
    return [bool]($active -and $active.pid -eq $steam.Id -and $active.ActiveUser -ne 0)
}

$steamExe = Find-SteamExe
$gameDir = Find-GameDir $steamExe
$launcher = Join-Path $gameDir 'ersc_launcher.exe'

if (Get-Process -Name eldenring -ErrorAction SilentlyContinue) {
    Write-Host 'Elden Ring is already running.'
    exit 0
}

if (Test-SteamLoggedIn) {
    Write-Host 'Steam is running and logged in.'
} elseif ($DryRun) {
    Write-Host "Would start Steam ($steamExe) if needed and wait for it to log in."
} else {
    if (Get-Process -Name steam -ErrorAction SilentlyContinue) {
        Write-Host 'Steam is running but not logged in yet.'
    } else {
        Write-Host "Starting Steam ($steamExe)."
        Start-Process -FilePath $steamExe -ArgumentList '-silent'
    }
    Write-Host 'Waiting for Steam to log in' -NoNewline
    $deadline = (Get-Date).AddSeconds(120)
    while (-not (Test-SteamLoggedIn)) {
        if ((Get-Date) -gt $deadline) {
            Write-Host ''
            Write-Warning "Steam hasn't logged in after 2 minutes. Log in to Steam, then run this again."
            exit 1
        }
        Start-Sleep -Seconds 2
        Write-Host '.' -NoNewline
    }
    Write-Host ' logged in.'
    # A moment for Steam's client service to finish coming up before the game's Steam API connects.
    Start-Sleep -Seconds 3
}

Write-Host "Starting Elden Ring with Seamless Co-op ($launcher)."
if ($DryRun) {
    Write-Host '(dry run: nothing started)'
    exit 0
}
Start-Process -FilePath $launcher -WorkingDirectory $gameDir
Start-Sleep -Seconds 15
if (Get-Process -Name eldenring -ErrorAction SilentlyContinue) {
    Write-Host 'Elden Ring is running.'
} else {
    Write-Warning "Elden Ring isn't running 15 seconds after launch. See 'If the game won't start' in README.md."
    exit 1
}
