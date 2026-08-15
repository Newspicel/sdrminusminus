param(
  [Parameter(Mandatory = $true)][string]$Prefix,
  [Parameter(Mandatory = $true)][string]$Sdk,
  [Parameter(Mandatory = $true)][string]$Destination
)
$ErrorActionPreference = "Stop"

$pins = @{}
Get-Content (Join-Path $PSScriptRoot "api.env") | ForEach-Object {
  if ($_ -match '^\s*([A-Z0-9_]+)="(.*)"\s*$') { $pins[$Matches[1]] = $Matches[2] }
}

$Destination = [System.IO.Path]::GetFullPath($Destination)
if (-not (Test-Path $Destination)) { throw "No module directory at $Destination" }
$import = Join-Path $Sdk "lib\sdrplay_api.lib"
if (-not (Test-Path $import)) { throw "No sdrplay_api.lib under $Sdk\lib - run fetch-api.ps1 first" }
if (-not (Test-Path (Join-Path $Sdk "include\sdrplay_api.h"))) { throw "No headers under $Sdk\include" }

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("sdrplay-module-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $work | Out-Null
try {
  $source = Join-Path $work "source"
  & git clone --quiet --branch $pins["MODULE_TAG"] --depth 1 https://github.com/pothosware/SoapySDRPlay3.git $source
  if ($LASTEXITCODE -ne 0) { throw "cloning SoapySDRPlay3 failed" }
  $commit = (& git -C $source rev-parse HEAD).Trim()
  if ($commit -ne $pins["MODULE_COMMIT"]) {
    throw "$($pins['MODULE_TAG']) resolves to $commit, not the pinned $($pins['MODULE_COMMIT'])"
  }

  $build = Join-Path $work "build"
  # The module's floor is CMake 2.8.12, which CMake 4 refuses to be compatible with.
  & cmake -S $source -B $build -A x64 `
    "-DCMAKE_PREFIX_PATH=$(Join-Path $Prefix 'Library')" `
    "-DLIBSDRPLAY_INCLUDE_DIRS=$(Join-Path $Sdk 'include')" `
    "-DLIBSDRPLAY_LIBRARIES=$import" `
    -DCMAKE_POLICY_VERSION_MINIMUM=3.5 | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "configuring SoapySDRPlay3 failed" }
  & cmake --build $build --config Release --parallel | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "building SoapySDRPlay3 failed" }

  $built = Get-ChildItem -Path $build -Recurse -Filter "*sdrPlaySupport*.dll" | Select-Object -First 1
  if (-not $built) { throw "SoapySDRPlay3 built no module" }
  Copy-Item $built.FullName $Destination
  if (Get-ChildItem $Destination -Filter "sdrplay_api.dll") {
    throw "the vendor library must not be staged: $Destination"
  }
  Write-Host "SDRplay module: $($built.Name) in $Destination"
} finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
