[CmdletBinding()]
param(
    [string]$SourceExe = 'target/release/md-preview.exe',
    [string]$OutputDirectory,
    [switch]$Force
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$manifest = Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Raw
$versionMatch = [regex]::Match($manifest, '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"')
if (-not $versionMatch.Success) { throw 'Cargo.toml package version missing.' }
$releaseVersion = $versionMatch.Groups[1].Value
if (-not [IO.Path]::IsPathRooted($SourceExe)) { $SourceExe = Join-Path $projectRoot $SourceExe }
$SourceExe = (Resolve-Path -LiteralPath $SourceExe).Path
if (-not $OutputDirectory) { $OutputDirectory = "release/v$releaseVersion" }
if (-not [IO.Path]::IsPathRooted($OutputDirectory)) { $OutputDirectory = Join-Path $projectRoot $OutputDirectory }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$guides = @(Get-ChildItem -LiteralPath (Join-Path $projectRoot 'docs') -File | Where-Object {
    $_.Name -match ('发布部署方案.*_v' + [regex]::Escape($releaseVersion) + '_\d{6}\.md$')
})
if ($guides.Count -ne 1) { throw "Expected exactly one deployment guide for v$releaseVersion." }
$releaseExe = Join-Path $OutputDirectory 'MD-Preview-windows-x64.exe'
if ((Test-Path -LiteralPath $releaseExe) -and -not $Force) { throw 'Package exists; use a new version or explicitly pass -Force for a local rebuild.' }
$actualVersion = (& $SourceExe --version | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $actualVersion -ne "Markdown Studio $releaseVersion") { throw "Executable version mismatch: $actualVersion" }
& (Join-Path $PSScriptRoot 'verify-windows-pe-dependencies.ps1') -Path $SourceExe
if ($LASTEXITCODE -ne 0) { throw 'PE dependency gate failed.' }
New-Item -ItemType Directory -Path (Join-Path $OutputDirectory 'licenses') -Force | Out-Null
Copy-Item -LiteralPath $SourceExe -Destination $releaseExe -Force
$exeHash = (Get-FileHash -LiteralPath $releaseExe -Algorithm SHA256).Hash
$exeBytes = (Get-Item -LiteralPath $releaseExe).Length
$signature = (Get-AuthenticodeSignature -LiteralPath $releaseExe).Status.ToString()
$guideText = Get-Content -LiteralPath $guides[0].FullName -Raw -Encoding utf8
$fields = [ordered]@{
    '(?m)^\| 实际大小 \|[^\r\n]*' = "| 实际大小 | $($exeBytes.ToString('N0', [Globalization.CultureInfo]::InvariantCulture)) 字节 |"
    '(?m)^\| SHA-256 \|[^\r\n]*' = "| SHA-256 | $exeHash |"
    '(?m)^\| Authenticode \|[^\r\n]*' = "| Authenticode | $signature |"
}
foreach ($field in $fields.GetEnumerator()) {
    if ([regex]::Matches($guideText, $field.Key).Count -ne 1) { throw "Missing or duplicated deployment field: $($field.Key)" }
    $guideText = [regex]::Replace($guideText, $field.Key, $field.Value)
}
$utf8 = [Text.UTF8Encoding]::new($false)
$releaseGuide = Join-Path $OutputDirectory $guides[0].Name
$publicGuide = Join-Path $OutputDirectory "MD-Studio-Deployment-v$releaseVersion-zh.md"
[IO.File]::WriteAllText($releaseGuide, $guideText, $utf8)
[IO.File]::WriteAllText($publicGuide, $guideText, $utf8)
Copy-Item -LiteralPath (Join-Path $projectRoot 'assets/editor/THIRD-PARTY-NOTICES.md') -Destination $OutputDirectory -Force
$licenseFiles = [ordered]@{
    'assets/editor/vditor/LICENSE' = 'Vditor-MIT.txt'
    'assets/editor/vditor/LUTE-LICENSE' = 'Lute-MulanPSL-2.0.txt'
    'assets/editor/vditor/KATEX-LICENSE' = 'KaTeX-MIT.txt'
    'LICENSE' = 'MD-Preview-MIT.txt'
}
foreach ($file in $licenseFiles.GetEnumerator()) {
    Copy-Item -LiteralPath (Join-Path $projectRoot $file.Key) -Destination (Join-Path $OutputDirectory ('licenses/' + $file.Value)) -Force
}
function Write-Checksum([string]$Path) {
    $digest = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    [IO.File]::WriteAllText(($Path + '.sha256'), ($digest + '  ' + [IO.Path]::GetFileName($Path) + "`n"), [Text.Encoding]::ASCII)
}
Write-Checksum $releaseExe
$archive = Join-Path $OutputDirectory "MD-Studio-v$releaseVersion-windows-x64.zip"
$licenseArchive = Join-Path $OutputDirectory "MD-Studio-v$releaseVersion-licenses.zip"
$packageFiles = @($releaseExe, ($releaseExe + '.sha256'), $releaseGuide,
    (Join-Path $OutputDirectory 'THIRD-PARTY-NOTICES.md'), (Join-Path $OutputDirectory 'licenses'))
Compress-Archive -LiteralPath $packageFiles -DestinationPath $archive -CompressionLevel Optimal -Force
Compress-Archive -LiteralPath @((Join-Path $OutputDirectory 'THIRD-PARTY-NOTICES.md'), (Join-Path $OutputDirectory 'licenses')) -DestinationPath $licenseArchive -CompressionLevel Optimal -Force
Write-Checksum $archive
Write-Checksum $licenseArchive
[pscustomobject]@{ Version = $releaseVersion; OutputDirectory = $OutputDirectory; ExeBytes = $exeBytes; ExeSha256 = $exeHash; Authenticode = $signature }
