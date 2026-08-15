param(
  [Parameter(Mandatory = $true)][string]$Destination
)
$ErrorActionPreference = "Stop"

$pins = @{}
Get-Content (Join-Path $PSScriptRoot "api.env") | ForEach-Object {
  if ($_ -match '^\s*([A-Z0-9_]+)="(.*)"\s*$') { $pins[$Matches[1]] = $Matches[2] }
}

if ([string]::IsNullOrWhiteSpace($Destination)) { throw "Refusing unsafe SDK destination: $Destination" }
$Destination = [System.IO.Path]::GetFullPath($Destination)
$workingDirectory = [System.IO.Path]::GetFullPath((Get-Location).Path)
$separator = [System.IO.Path]::DirectorySeparatorChar
if ($workingDirectory -eq $Destination -or
    $workingDirectory.StartsWith($Destination.TrimEnd($separator) + $separator, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Refusing SDK destination at or above the working directory: $Destination"
}
if (Test-Path $Destination) { Remove-Item -Recurse -Force $Destination }
$include = Join-Path $Destination "include"
$lib = Join-Path $Destination "lib"
$work = Join-Path $Destination ".work"
New-Item -ItemType Directory -Force $include, $lib, $work | Out-Null

$file = Join-Path $work $pins["WINDOWS_FILE"]
Write-Host "Fetching the SDRplay API $($pins['API_VERSION']) SDK ($($pins['WINDOWS_FILE']))."
Write-Host "Use of it is governed by SDRplay's end user licence agreement; it is a build input only."
Invoke-WebRequest -Uri $pins["WINDOWS_URL"] -OutFile $file -UseBasicParsing -MaximumRedirection 10

$digest = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant()
if ($digest -ne $pins["WINDOWS_SHA256"]) {
  Write-Host "expected $($pins['WINDOWS_SHA256'])"
  Write-Host "received $digest"
  Write-Host "SDRplay has published a new API. Re-pin packaging/sdrplay/api.env after checking the"
  Write-Host "new version against the module requirements in docs/src/hardware.md."
  throw "$($pins['WINDOWS_FILE']) does not match the pinned digest."
}

# innoextract rather than a silent run of the installer: the payload is all this build needs, and
# installing would register the vendor service and its USB driver on the build machine.
& innoextract --extract --silent --output-dir $work $file
if ($LASTEXITCODE -ne 0) { throw "innoextract failed on $($pins['WINDOWS_FILE'])" }

$header = Get-ChildItem -Path $work -Recurse -Filter "sdrplay_api.h" | Select-Object -First 1
if (-not $header) { throw "no sdrplay_api.h in $($pins['WINDOWS_FILE'])" }
# The 32-bit import library carries the same name in a sibling directory, so the architecture is
# chosen by path rather than by name.
$import = Get-ChildItem -Path $work -Recurse -Filter "sdrplay_api.lib" |
  Where-Object { $_.DirectoryName -match "x64" } | Select-Object -First 1
if (-not $import) { throw "no x64 sdrplay_api.lib in $($pins['WINDOWS_FILE'])" }

Copy-Item (Join-Path $header.DirectoryName "sdrplay_api*.h") $include
Copy-Item $import.FullName $lib
Remove-Item -Recurse -Force $work
Write-Host "SDRplay SDK: sdrplay_api.lib and $((Get-ChildItem $include -Filter '*.h').Count) headers in $Destination"
