param(
  [Parameter(Mandatory = $true)][string]$Prefix,
  [Parameter(Mandatory = $true)][string]$Destination
)
$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($Destination) -or $Destination -eq "." -or $Destination -eq "\") {
  throw "Refusing unsafe staging destination: $Destination"
}
if (Test-Path $Destination) { Remove-Item -Recurse -Force $Destination }
$bin = Join-Path $Destination "bin"
$modules = Join-Path $Destination "lib\SoapySDR\modules0.8"
$licenses = Join-Path $Destination "licenses"
New-Item -ItemType Directory -Force $bin, $modules, $licenses | Out-Null

Get-ChildItem (Join-Path $Prefix "Library\bin") -Filter "*.dll" | Copy-Item -Destination $bin
$sourceModules = Join-Path $Prefix "Library\lib\SoapySDR\modules0.8"
$pattern = "rtlsdr|hackrf|airspy|bladerf|lms7|pluto|remote"
Get-ChildItem $sourceModules -Filter "*.dll" | Where-Object { $_.Name -match $pattern } |
  Copy-Item -Destination $modules
if (-not (Get-ChildItem $modules | Where-Object { $_.Name -match "rtlsdr" })) { throw "SoapyRTLSDR was not staged" }
if (-not (Get-ChildItem $modules | Where-Object { $_.Name -match "hackrf" })) { throw "SoapyHackRF was not staged" }
if (Test-Path (Join-Path $Prefix "conda-meta")) {
  Copy-Item (Join-Path $Prefix "conda-meta\*.json") $licenses
}
foreach ($licenseRoot in @("Library\share\licenses", "share\licenses")) {
  $source = Join-Path $Prefix $licenseRoot
  if (Test-Path $source) {
    Copy-Item $source (Join-Path $licenses "texts") -Recurse -Force
  }
}
