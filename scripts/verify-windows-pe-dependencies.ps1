[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Path = "target\release\md-preview.exe",

    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Assert-ByteRange {
    param(
        [byte[]]$Bytes,
        [long]$Offset,
        [int]$Length
    )

    if ($Offset -lt 0 -or $Length -lt 0 -or ($Offset + $Length) -gt $Bytes.LongLength) {
        throw "PE structure points outside the file (offset=$Offset, length=$Length)."
    }
}

function Read-UInt16LE {
    param([byte[]]$Bytes, [long]$Offset)
    Assert-ByteRange -Bytes $Bytes -Offset $Offset -Length 2
    return [BitConverter]::ToUInt16($Bytes, [int]$Offset)
}

function Read-UInt32LE {
    param([byte[]]$Bytes, [long]$Offset)
    Assert-ByteRange -Bytes $Bytes -Offset $Offset -Length 4
    return [BitConverter]::ToUInt32($Bytes, [int]$Offset)
}

function Read-UInt64LE {
    param([byte[]]$Bytes, [long]$Offset)
    Assert-ByteRange -Bytes $Bytes -Offset $Offset -Length 8
    return [BitConverter]::ToUInt64($Bytes, [int]$Offset)
}

function Read-AsciiZ {
    param([byte[]]$Bytes, [long]$Offset)

    Assert-ByteRange -Bytes $Bytes -Offset $Offset -Length 1
    $end = $Offset
    $limit = [Math]::Min($Bytes.LongLength, $Offset + 4096)
    while ($end -lt $limit -and $Bytes[[int]$end] -ne 0) {
        $end++
    }
    if ($end -eq $limit) {
        throw "Unterminated PE import name at file offset $Offset."
    }
    return [Text.Encoding]::ASCII.GetString($Bytes, [int]$Offset, [int]($end - $Offset))
}

function Convert-RvaToFileOffset {
    param(
        [uint32]$Rva,
        [uint32]$SizeOfHeaders,
        [object[]]$Sections
    )

    if ([uint64]$Rva -lt [uint64]$SizeOfHeaders) {
        return [long]$Rva
    }

    foreach ($section in $Sections) {
        $span = if ([uint64]$section.VirtualSize -gt [uint64]$section.RawSize) {
            [uint64]$section.VirtualSize
        }
        else {
            [uint64]$section.RawSize
        }
        $start = [uint64]$section.VirtualAddress
        $end = $start + $span
        if ([uint64]$Rva -ge $start -and [uint64]$Rva -lt $end) {
            return [long]([uint64]$section.RawPointer + ([uint64]$Rva - $start))
        }
    }

    throw ("Unable to map PE RVA 0x{0:X8} to a file offset." -f $Rva)
}

function Get-PeDataDirectory {
    param(
        [byte[]]$Bytes,
        [long]$OptionalHeaderOffset,
        [bool]$IsPe32Plus,
        [uint32]$DirectoryCount,
        [int]$Index
    )

    if ($Index -ge $DirectoryCount) {
        return [pscustomobject]@{ Rva = [uint32]0; Size = [uint32]0 }
    }

    $directoryOffset = $OptionalHeaderOffset + $(if ($IsPe32Plus) { 112 } else { 96 }) + ($Index * 8)
    return [pscustomobject]@{
        Rva  = Read-UInt32LE -Bytes $Bytes -Offset $directoryOffset
        Size = Read-UInt32LE -Bytes $Bytes -Offset ($directoryOffset + 4)
    }
}

function Get-PeImportedDlls {
    param([string]$FilePath)

    $resolved = Resolve-Path -LiteralPath $FilePath -ErrorAction Stop
    $bytes = [IO.File]::ReadAllBytes($resolved.Path)
    if ($bytes.LongLength -lt 0x40 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
        throw "Not a valid PE file: $($resolved.Path)"
    }

    $peOffset = [long](Read-UInt32LE -Bytes $bytes -Offset 0x3C)
    if ((Read-UInt32LE -Bytes $bytes -Offset $peOffset) -ne 0x00004550) {
        throw "PE signature is missing: $($resolved.Path)"
    }

    $sectionCount = Read-UInt16LE -Bytes $bytes -Offset ($peOffset + 6)
    $optionalHeaderSize = Read-UInt16LE -Bytes $bytes -Offset ($peOffset + 20)
    $optionalHeaderOffset = $peOffset + 24
    $magic = Read-UInt16LE -Bytes $bytes -Offset $optionalHeaderOffset
    $isPe32Plus = switch ($magic) {
        0x20B { $true }
        0x10B { $false }
        default { throw ("Unsupported PE optional-header magic: 0x{0:X4}" -f $magic) }
    }
    $directoryCountOffset = $optionalHeaderOffset + $(if ($isPe32Plus) { 108 } else { 92 })
    $directoryCount = Read-UInt32LE -Bytes $bytes -Offset $directoryCountOffset
    $sizeOfHeaders = Read-UInt32LE -Bytes $bytes -Offset ($optionalHeaderOffset + 60)
    $imageBase = if ($isPe32Plus) {
        Read-UInt64LE -Bytes $bytes -Offset ($optionalHeaderOffset + 24)
    }
    else {
        [uint64](Read-UInt32LE -Bytes $bytes -Offset ($optionalHeaderOffset + 28))
    }

    $sections = @()
    $sectionTableOffset = $optionalHeaderOffset + $optionalHeaderSize
    for ($index = 0; $index -lt $sectionCount; $index++) {
        $sectionOffset = $sectionTableOffset + ($index * 40)
        $sections += [pscustomobject]@{
            VirtualSize    = Read-UInt32LE -Bytes $bytes -Offset ($sectionOffset + 8)
            VirtualAddress = Read-UInt32LE -Bytes $bytes -Offset ($sectionOffset + 12)
            RawSize        = Read-UInt32LE -Bytes $bytes -Offset ($sectionOffset + 16)
            RawPointer     = Read-UInt32LE -Bytes $bytes -Offset ($sectionOffset + 20)
        }
    }

    $imports = [Collections.Generic.List[string]]::new()

    # IMAGE_DIRECTORY_ENTRY_IMPORT (descriptor size 20, DLL name RVA at +12).
    $importDirectory = Get-PeDataDirectory -Bytes $bytes -OptionalHeaderOffset $optionalHeaderOffset `
        -IsPe32Plus $isPe32Plus -DirectoryCount $directoryCount -Index 1
    if ($importDirectory.Rva -ne 0) {
        $descriptorOffset = Convert-RvaToFileOffset -Rva $importDirectory.Rva `
            -SizeOfHeaders $sizeOfHeaders -Sections $sections
        for ($index = 0; $index -lt 4096; $index++) {
            $current = $descriptorOffset + ($index * 20)
            $originalThunk = Read-UInt32LE -Bytes $bytes -Offset $current
            $nameRva = Read-UInt32LE -Bytes $bytes -Offset ($current + 12)
            $firstThunk = Read-UInt32LE -Bytes $bytes -Offset ($current + 16)
            if ($originalThunk -eq 0 -and $nameRva -eq 0 -and $firstThunk -eq 0) {
                break
            }
            if ($nameRva -ne 0) {
                $nameOffset = Convert-RvaToFileOffset -Rva $nameRva `
                    -SizeOfHeaders $sizeOfHeaders -Sections $sections
                $imports.Add((Read-AsciiZ -Bytes $bytes -Offset $nameOffset)) | Out-Null
            }
        }
    }

    # IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT (descriptor size 32, DLL name at +4).
    $delayDirectory = Get-PeDataDirectory -Bytes $bytes -OptionalHeaderOffset $optionalHeaderOffset `
        -IsPe32Plus $isPe32Plus -DirectoryCount $directoryCount -Index 13
    if ($delayDirectory.Rva -ne 0) {
        $descriptorOffset = Convert-RvaToFileOffset -Rva $delayDirectory.Rva `
            -SizeOfHeaders $sizeOfHeaders -Sections $sections
        for ($index = 0; $index -lt 4096; $index++) {
            $current = $descriptorOffset + ($index * 32)
            $attributes = Read-UInt32LE -Bytes $bytes -Offset $current
            $nameValue = Read-UInt32LE -Bytes $bytes -Offset ($current + 4)
            if ($attributes -eq 0 -and $nameValue -eq 0) {
                break
            }
            if ($nameValue -ne 0) {
                $nameRva64 = [uint64]$nameValue
                if (($attributes -band 1) -eq 0) {
                    if ($nameRva64 -lt $imageBase) {
                        throw "Invalid VA-based delay-import name."
                    }
                    $nameRva64 -= $imageBase
                }
                if ($nameRva64 -gt [uint32]::MaxValue) {
                    throw "Delay-import name RVA exceeds the PE32 range."
                }
                $nameOffset = Convert-RvaToFileOffset -Rva ([uint32]$nameRva64) `
                    -SizeOfHeaders $sizeOfHeaders -Sections $sections
                $imports.Add((Read-AsciiZ -Bytes $bytes -Offset $nameOffset)) | Out-Null
            }
        }
    }

    return @($imports | Sort-Object -Unique)
}

function Test-ForbiddenVcRuntimeImport {
    param([string]$DllName)
    return $DllName.ToUpperInvariant() -match '^(VCRUNTIME|MSVCP|CONCRT)'
}

function Invoke-SelfTest {
    $cases = @(
        @{ Name = "VCRUNTIME140.dll"; Forbidden = $true },
        @{ Name = "vcruntime140_1.dll"; Forbidden = $true },
        @{ Name = "MSVCP140.dll"; Forbidden = $true },
        @{ Name = "CONCRT140.dll"; Forbidden = $true },
        @{ Name = "ucrtbase.dll"; Forbidden = $false },
        @{ Name = "KERNEL32.dll"; Forbidden = $false }
    )
    foreach ($case in $cases) {
        $actual = Test-ForbiddenVcRuntimeImport -DllName $case.Name
        if ($actual -ne $case.Forbidden) {
            throw "Self-test failed for $($case.Name): expected $($case.Forbidden), got $actual."
        }
    }
    Write-Host "PE dependency gate self-test passed ($($cases.Count) cases)."
}

if ($SelfTest) {
    Invoke-SelfTest
    exit 0
}

try {
    $resolvedPath = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $imports = @(Get-PeImportedDlls -FilePath $resolvedPath)
    $forbidden = @($imports | Where-Object { Test-ForbiddenVcRuntimeImport -DllName $_ })

    Write-Host "PE imports: $resolvedPath"
    foreach ($import in $imports) {
        Write-Host "  $import"
    }

    if ($forbidden.Count -gt 0) {
        Write-Host "Forbidden dynamic VC runtime imports detected: $($forbidden -join ', ')" -ForegroundColor Red
        exit 1
    }

    Write-Host "PE dependency gate passed: no VCRUNTIME*, MSVCP*, or CONCRT* imports."
}
catch {
    Write-Host "PE dependency gate failed: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

exit 0
