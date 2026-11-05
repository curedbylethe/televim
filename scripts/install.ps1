[CmdletBinding()]
param(
    [string] $Version = $env:TELEVIM_VERSION,
    [string] $InstallDir = $env:TELEVIM_INSTALL_DIR,
    [switch] $NoModifyPath,
    [switch] $SkipVersionCheck,
    [switch] $Help
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

if ($Help) {
    @'
Install Televim for Windows x86_64.

Usage: install.ps1 [-Version <version>] [-InstallDir <path>] [-NoModifyPath]

The unsigned executable may trigger SmartScreen. Restart the terminal after
installation so the updated User PATH takes effect.
'@ | Write-Output
    exit 0
}

if ($env:OS -ne 'Windows_NT') {
    throw 'This installer supports Windows x86_64 only.'
}

$architecture = $env:PROCESSOR_ARCHITEW6432
if ([string]::IsNullOrWhiteSpace($architecture)) {
    $architecture = $env:PROCESSOR_ARCHITECTURE
}
if ($architecture -notin @('AMD64', 'x86_64')) {
    throw "Unsupported Windows architecture '$architecture'. Supported target: x86_64-pc-windows-msvc."
}

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA 'televim\bin'
}
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$baseUrl = $env:TELEVIM_BASE_URL
if ([string]::IsNullOrWhiteSpace($baseUrl)) {
    $baseUrl = 'https://github.com/curedbylethe/televim/releases/download'
}
$baseUrl = $baseUrl.TrimEnd('/')

if ([string]::IsNullOrWhiteSpace($Version)) {
    try {
        $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/curedbylethe/televim/releases/latest' -Headers @{ 'User-Agent' = 'televim-installer' }
        $Version = [string] $release.tag_name
    }
    catch {
        throw "Could not resolve the latest Televim release: $($_.Exception.Message)"
    }
}
$Version = $Version -replace '^v', ''
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$') {
    throw "Invalid Televim version '$Version'. Expected X.Y.Z or X.Y.Z-prerelease."
}

$asset = "televim-$Version-x86_64-pc-windows-msvc.zip"
$releaseUrl = "$baseUrl/v$Version"
$archiveUrl = "$releaseUrl/$asset"
$checksumsUrl = "$releaseUrl/SHA256SUMS"
$temporary = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporary | Out-Null

try {
    $archivePath = Join-Path $temporary $asset
    $checksumsPath = Join-Path $temporary 'SHA256SUMS'
    foreach ($download in @(@{ Url = $archiveUrl; Path = $archivePath }, @{ Url = $checksumsUrl; Path = $checksumsPath })) {
        try {
            Invoke-WebRequest -UseBasicParsing -Uri $download.Url -OutFile $download.Path
        }
        catch {
            throw "Could not download $($download.Url): $($_.Exception.Message)"
        }
    }

    $checksumPattern = '^([a-fA-F0-9]{64})\s+\*?' + [regex]::Escape($asset) + '$'
    $checksumLines = @(Get-Content -LiteralPath $checksumsPath | Where-Object { $_ -match $checksumPattern })
    if ($checksumLines.Count -ne 1) {
        throw "SHA256SUMS must contain exactly one entry for $asset."
    }
    $expectedHash = [regex]::Match($checksumLines[0], $checksumPattern).Groups[1].Value
    $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
    if (-not [string]::Equals($expectedHash, $actualHash, [StringComparison]::OrdinalIgnoreCase)) {
        throw "SHA-256 mismatch for $asset. No files were installed."
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($archivePath)
    try {
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName -replace '\\', '/'
            if ($name.StartsWith('/') -or $name -match '^[A-Za-z]:' -or ($name -split '/') -contains '..') {
                throw "Unsafe path in $asset`: $($entry.FullName)"
            }
        }
    }
    finally {
        $zip.Dispose()
    }

    $extractDir = Join-Path $temporary 'extract'
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir
    $expectedFiles = @('LICENSE-APACHE', 'LICENSE-MIT', 'README.md', 'televim.exe')
    $actualFiles = @(Get-ChildItem -LiteralPath $extractDir -File -Recurse | ForEach-Object {
        $_.FullName.Substring($extractDir.Length).TrimStart([char[]]@('\', '/')).Replace('\', '/')
    } | Sort-Object)
    if (Compare-Object ($expectedFiles | Sort-Object) $actualFiles) {
        throw "Unexpected files in $asset`: $($actualFiles -join ', ')"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -LiteralPath (Join-Path $extractDir 'televim.exe') -Destination (Join-Path $InstallDir 'televim.exe') -Force

    if (-not $NoModifyPath) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $pathEntries = @($userPath -split ';' | ForEach-Object { $_.Trim().TrimEnd([char[]]@('\')) } | Where-Object { $_ })
        if (-not ($pathEntries | Where-Object { [string]::Equals($_, $InstallDir.TrimEnd([char[]]@('\')), [StringComparison]::OrdinalIgnoreCase) })) {
            $newUserPath = (@($pathEntries) + $InstallDir) -join ';'
            try {
                [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
            }
            catch {
                Write-Warning "Could not update User PATH. Add this directory manually: $InstallDir"
            }
        }
    }

    if (-not $SkipVersionCheck) {
        $versionOutput = & (Join-Path $InstallDir 'televim.exe') --version
        if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch '^televim\s+' -or $versionOutput -notmatch [regex]::Escape($Version)) {
            throw "Installed binary could not verify version $Version. Check architecture and Windows runtime requirements."
        }
    }

    Write-Output "Installed Televim to $(Join-Path $InstallDir 'televim.exe')."
    if ($NoModifyPath) {
        Write-Output "Add $InstallDir to User PATH, then restart the terminal."
    }
    else {
        Write-Output 'Restart the terminal so the updated User PATH takes effect.'
    }
    Write-Output 'Run: televim --version'
    Write-Output 'The unsigned executable may trigger SmartScreen.'
}
finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
