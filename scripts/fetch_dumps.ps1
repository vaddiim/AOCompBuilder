<#
.SYNOPSIS
  Downloads the game data dumps the catalog is built from into assets/data.
.DESCRIPTION
  Fetches items.json, spells.json and items.txt from ao-bin-dumps. Existing files are kept
  unless -Force is given. After a refresh, delete assets/data/catalog.json (or run `cargo test`)
  to rebuild the catalog. spell_loc.json is derived separately from localization.json:
  see the `distil_localization` test in src/catalog.rs.
.PARAMETER Force  Re-download files that already exist.
#>
[CmdletBinding()]
param([switch]$Force)
$ErrorActionPreference = 'Stop'
$base = 'https://raw.githubusercontent.com/ao-data/ao-bin-dumps/master'
$dest = Join-Path (Split-Path -Parent $PSScriptRoot) 'assets/data'
New-Item -ItemType Directory -Force $dest | Out-Null

$files = @{
    'items.json' = 'items.json'
    'spells.json' = 'spells.json'
    'items.txt'  = 'formatted/items.txt'
}
foreach ($name in $files.Keys) {
    $path = Join-Path $dest $name
    if ((Test-Path $path) -and -not $Force) { Write-Host "skip $name (exists)"; continue }
    $part = "$path.part"
    Write-Host "fetch $name"
    for ($try = 1; ; $try++) {
        try {
            Invoke-WebRequest -Uri "$base/$($files[$name])" -OutFile $part -TimeoutSec 300
            Move-Item $part $path -Force
            break
        } catch {
            Remove-Item $part -ErrorAction SilentlyContinue
            if ($try -ge 3) { throw }
            Start-Sleep -Seconds (3 * $try)
        }
    }
}
