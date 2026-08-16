param(
  [Parameter(Mandatory = $true)][string]$Prefix,
  [Parameter(Mandatory = $true)][string]$Destination
)
$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($Destination)) {
  throw "Refusing unsafe staging destination: $Destination"
}
$Destination = [System.IO.Path]::GetFullPath($Destination)
$workingDirectory = [System.IO.Path]::GetFullPath((Get-Location).Path)
$volumeRoot = [System.IO.Path]::GetPathRoot($Destination)
$separator = [System.IO.Path]::DirectorySeparatorChar
$atRoot = $Destination.TrimEnd($separator) -eq $volumeRoot.TrimEnd($separator)
$atOrAboveWorkingDirectory = $workingDirectory -eq $Destination -or
  $workingDirectory.StartsWith($Destination.TrimEnd($separator) + $separator, [System.StringComparison]::OrdinalIgnoreCase)
if ($atRoot -or $atOrAboveWorkingDirectory) {
  throw "Refusing staging destination at or above the working directory: $Destination"
}
if (Test-Path $Destination) { Remove-Item -Recurse -Force $Destination }
$bin = Join-Path $Destination "bin"
$modules = Join-Path $Destination "lib\SoapySDR\modules0.8"
$licenses = Join-Path $Destination "licenses"
New-Item -ItemType Directory -Force $bin, $modules, $licenses | Out-Null

Get-ChildItem (Join-Path $Prefix "Library\bin") -Filter "*.dll" | Copy-Item -Destination $bin
$sourceModules = Join-Path $Prefix "Library\lib\SoapySDR\modules0.8"
$pattern = "airspy|bladerf|lms7|pluto|remote"
Get-ChildItem $sourceModules -Filter "*.dll" | Where-Object { $_.Name -match $pattern } |
  Copy-Item -Destination $modules
if (-not (Get-ChildItem $modules | Where-Object { $_.Name -match "airspy" })) { throw "SoapyAirspy was not staged" }
if (Test-Path (Join-Path $Prefix "conda-meta")) {
  Copy-Item (Join-Path $Prefix "conda-meta\*.json") $licenses
}
foreach ($licenseRoot in @("Library\share\licenses", "share\licenses")) {
  $source = Join-Path $Prefix $licenseRoot
  if (Test-Path $source) {
    Copy-Item $source (Join-Path $licenses "texts") -Recurse -Force
  }
}
