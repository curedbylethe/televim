$ErrorActionPreference = 'Stop'
$installer = Join-Path $PSScriptRoot 'install.ps1'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString('N'))
$server = $null
$oldArchitecture = $env:PROCESSOR_ARCHITECTURE
$oldArchitectureW6432 = $env:PROCESSOR_ARCHITEW6432
$oldBaseUrl = $env:TELEVIM_BASE_URL
$oldVersion = $env:TELEVIM_VERSION
$oldInstallDir = $env:TELEVIM_INSTALL_DIR
$environmentKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false)
$oldUserPath = if ($environmentKey) {
    $value = $environmentKey.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if ($null -ne $value) { @{ Exists = $true; Value = $value; Kind = $environmentKey.GetValueKind('Path') } }
    else { @{ Exists = $false } }
} else { @{ Exists = $false } }
if ($environmentKey) { $environmentKey.Dispose() }

function Get-RawUserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false)
    if (-not $key) { return $null }
    try { return [string] $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) }
    finally { $key.Dispose() }
}

function Set-RawUserPath($value) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try { $key.SetValue('Path', $value, [Microsoft.Win32.RegistryValueKind]::ExpandString) }
    finally { $key.Dispose() }
}

function Assert($condition, $message) {
    if (-not $condition) { throw $message }
}

function Invoke-Installer($version, $installDir) {
    & $installer -Version $version -InstallDir $installDir -NoModifyPath -SkipVersionCheck
}

function Invoke-InstallerExpression {
    Get-Content -LiteralPath $installer -Raw | Invoke-Expression
}

function Assert-InstallerFails($version, $installDir, $expectedText) {
    try {
        $output = (& $installer -Version $version -InstallDir $installDir -NoModifyPath -SkipVersionCheck 2>&1 | Out-String)
        throw "Expected installer to fail for $version; output: $output"
    }
    catch {
        $message = $_ | Out-String
        if ($message -notlike "*$expectedText*") { throw "Expected error containing '$expectedText'; got: $message" }
    }
}

try {
    New-Item -ItemType Directory -Force -Path $temporary | Out-Null
    $versionDir = Join-Path $temporary 'releases/download/v9.9.9'
    New-Item -ItemType Directory -Force -Path $versionDir | Out-Null
    $package = Join-Path $temporary 'package'
    New-Item -ItemType Directory -Force -Path $package | Out-Null
    Set-Content -LiteralPath (Join-Path $package 'televim.exe') -Value 'fixture executable'
    Set-Content -LiteralPath (Join-Path $package 'README.md') -Value 'fixture readme'
    Set-Content -LiteralPath (Join-Path $package 'LICENSE-MIT') -Value 'MIT'
    Set-Content -LiteralPath (Join-Path $package 'LICENSE-APACHE') -Value 'Apache'
    $asset = 'televim-9.9.9-x86_64-pc-windows-msvc.zip'
    $archive = Join-Path $versionDir $asset
    Compress-Archive -Path (Join-Path $package '*') -DestinationPath $archive
    $digest = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $versionDir 'SHA256SUMS') -Value "$digest  $asset"

    $port = [int] (& python -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
    $server = Start-Process -FilePath python -ArgumentList "-m http.server $port --bind 127.0.0.1 --directory `"$temporary`"" -PassThru -WindowStyle Hidden
    $env:TELEVIM_BASE_URL = "http://127.0.0.1:$port/releases/download"
    $ready = $false
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$env:TELEVIM_BASE_URL/v9.9.9/SHA256SUMS" | Out-Null
            $ready = $true
            break
        }
        catch { Start-Sleep -Milliseconds 200 }
    }
    Assert $ready 'Local HTTP fixture did not start.'

    $installDir = Join-Path $temporary 'bin'
    $userPathBefore = Get-RawUserPath
    $installOutput = Invoke-Installer '9.9.9' $installDir | Out-String
    $installed = Join-Path $installDir 'televim.exe'
    Assert (Test-Path -LiteralPath $installed) 'Installer did not copy televim.exe.'
    Assert ($installOutput.Contains('Installed Televim')) 'Installer did not report success after checksum verification.'
    Assert ((Get-RawUserPath) -eq $userPathBefore) '-NoModifyPath changed the User PATH.'
    Invoke-Installer '9.9.9' $installDir | Out-Null
    Assert ((Get-ChildItem -LiteralPath $installDir -File).Count -eq 1) 'Repeated install left extra files.'

    $pathInstallDir = Join-Path $temporary 'path-bin'
    $pathSeed = '%USERPROFILE%\existing-path;;C:\tools\'
    Set-RawUserPath $pathSeed
    $pathInstallOutput = (& $installer -Version '9.9.9' -InstallDir $pathInstallDir -SkipVersionCheck 3>&1 | Out-String)
    Assert ($pathInstallOutput -notmatch '(?i)(could not broadcast|did not receive)') 'Installer failed to notify Windows after changing User PATH.'
    $pathAfterFirstInstall = Get-RawUserPath
    $pathEntries = @($pathAfterFirstInstall -split ';' | ForEach-Object { $_.Trim().TrimEnd([char[]]@('\')) } | Where-Object { $_ })
    Assert (@($pathEntries | Where-Object { [string]::Equals($_, $pathInstallDir, [StringComparison]::OrdinalIgnoreCase) }).Count -eq 1) 'Installer did not add its directory to User PATH once.'
    & $installer -Version '9.9.9' -InstallDir $pathInstallDir -SkipVersionCheck | Out-Null
    $pathAfterSecondInstall = Get-RawUserPath
    $pathEntries = @($pathAfterSecondInstall -split ';' | ForEach-Object { $_.Trim().TrimEnd([char[]]@('\')) } | Where-Object { $_ })
    Assert (@($pathEntries | Where-Object { [string]::Equals($_, $pathInstallDir, [StringComparison]::OrdinalIgnoreCase) }).Count -eq 1) 'Repeated install duplicated its User PATH entry.'
    Assert ($pathAfterSecondInstall.StartsWith("$pathSeed;", [StringComparison]::Ordinal)) 'Installer changed the existing User PATH text.'
    Assert ($pathEntries[0] -eq '%USERPROFILE%\existing-path') 'Installer expanded an existing User PATH variable.'

    $badDir = Join-Path $temporary 'bad-checksum-bin'
    Set-Content -LiteralPath (Join-Path $versionDir 'SHA256SUMS') -Value (('0' * 64) + "  $asset")
    Assert-InstallerFails '9.9.9' $badDir 'SHA-256 mismatch'
    Assert (-not (Test-Path -LiteralPath (Join-Path $badDir 'televim.exe'))) 'Checksum failure installed the binary.'

    $iexDir = Join-Path $temporary 'iex-bin'
    $env:TELEVIM_VERSION = '9.9.9'
    $env:TELEVIM_INSTALL_DIR = $iexDir
    try {
        $iexOutput = (Invoke-InstallerExpression 2>&1 | Out-String)
        throw "Expected the documented Invoke-Expression form to report checksum failure; output: $iexOutput"
    }
    catch {
        if (($_ | Out-String) -notlike '*SHA-256 mismatch*') { throw }
    }
    Assert (-not (Test-Path -LiteralPath (Join-Path $iexDir 'televim.exe'))) 'Invoke-Expression checksum failure installed the binary.'
    if ($null -eq $oldVersion) { Remove-Item Env:TELEVIM_VERSION -ErrorAction SilentlyContinue }
    else { $env:TELEVIM_VERSION = $oldVersion }
    if ($null -eq $oldInstallDir) { Remove-Item Env:TELEVIM_INSTALL_DIR -ErrorAction SilentlyContinue }
    else { $env:TELEVIM_INSTALL_DIR = $oldInstallDir }

    $unsafeVersionDir = Join-Path $temporary 'releases/download/v9.9.8'
    New-Item -ItemType Directory -Force -Path $unsafeVersionDir | Out-Null
    $unsafeAsset = 'televim-9.9.8-x86_64-pc-windows-msvc.zip'
    $unsafeArchive = Join-Path $unsafeVersionDir $unsafeAsset
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::Open($unsafeArchive, [IO.Compression.ZipArchiveMode]::Create)
    try { $zip.CreateEntry('../escape.txt') | Out-Null }
    finally { $zip.Dispose() }
    $unsafeDigest = (Get-FileHash -LiteralPath $unsafeArchive -Algorithm SHA256).Hash
    Set-Content -LiteralPath (Join-Path $unsafeVersionDir 'SHA256SUMS') -Value "$unsafeDigest  $unsafeAsset"
    Assert-InstallerFails '9.9.8' (Join-Path $temporary 'unsafe-bin') 'Unsafe path'
    Assert (-not (Test-Path -LiteralPath (Join-Path $temporary 'escape.txt'))) 'Unsafe archive wrote outside its extraction directory.'

    $env:PROCESSOR_ARCHITECTURE = 'ARM64'
    $env:PROCESSOR_ARCHITEW6432 = 'ARM64'
    Assert-InstallerFails '9.9.9' (Join-Path $temporary 'arm-bin') 'x86_64-pc-windows-msvc'
    if ($null -eq $oldArchitecture) { Remove-Item Env:PROCESSOR_ARCHITECTURE -ErrorAction SilentlyContinue }
    else { $env:PROCESSOR_ARCHITECTURE = $oldArchitecture }
    if ($null -eq $oldArchitectureW6432) { Remove-Item Env:PROCESSOR_ARCHITEW6432 -ErrorAction SilentlyContinue }
    else { $env:PROCESSOR_ARCHITEW6432 = $oldArchitectureW6432 }

    Assert-InstallerFails '9.9.7' (Join-Path $temporary 'missing-bin') 'http://127.0.0.1:'

    $source = Get-Content -LiteralPath $installer -Raw
    Assert ($source -notmatch '(?i)\bsetx\b') 'Installer must not use setx.'
    Assert ($source.Contains('DoNotExpandEnvironmentNames') -and $source.Contains('RegistryValueKind]::ExpandString') -and $source.Contains('SendMessageTimeout') -and $source.Contains('OrdinalIgnoreCase') -and $source -notmatch '\[CmdletBinding\]') 'Installer invocation and User PATH update must preserve and announce raw existing entries.'
    Write-Output 'PowerShell installer tests passed.'
}
finally {
    if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue }
    if ($oldUserPath.Exists) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
        try { $key.SetValue('Path', $oldUserPath.Value, $oldUserPath.Kind) }
        finally { $key.Dispose() }
    } else {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        if ($key) { try { $key.DeleteValue('Path', $false) } finally { $key.Dispose() } }
    }
    if ($null -eq $oldArchitecture) { Remove-Item Env:PROCESSOR_ARCHITECTURE -ErrorAction SilentlyContinue }
    else { $env:PROCESSOR_ARCHITECTURE = $oldArchitecture }
    if ($null -eq $oldArchitectureW6432) { Remove-Item Env:PROCESSOR_ARCHITEW6432 -ErrorAction SilentlyContinue }
    else { $env:PROCESSOR_ARCHITEW6432 = $oldArchitectureW6432 }
    if ($null -eq $oldBaseUrl) { Remove-Item Env:TELEVIM_BASE_URL -ErrorAction SilentlyContinue }
    else { $env:TELEVIM_BASE_URL = $oldBaseUrl }
    if ($null -eq $oldVersion) { Remove-Item Env:TELEVIM_VERSION -ErrorAction SilentlyContinue }
    else { $env:TELEVIM_VERSION = $oldVersion }
    if ($null -eq $oldInstallDir) { Remove-Item Env:TELEVIM_INSTALL_DIR -ErrorAction SilentlyContinue }
    else { $env:TELEVIM_INSTALL_DIR = $oldInstallDir }
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
