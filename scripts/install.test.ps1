$ErrorActionPreference = 'Stop'
$installer = Join-Path $PSScriptRoot 'install.ps1'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString('N'))
$server = $null
$oldArchitecture = $env:PROCESSOR_ARCHITECTURE
$oldArchitectureW6432 = $env:PROCESSOR_ARCHITEW6432
$oldBaseUrl = $env:TELEVIM_BASE_URL

function Assert($condition, $message) {
    if (-not $condition) { throw $message }
}

function Invoke-Installer($version, $installDir) {
    & $installer -Version $version -InstallDir $installDir -NoModifyPath -SkipVersionCheck
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
    $userPathBefore = [Environment]::GetEnvironmentVariable('Path', 'User')
    $installOutput = Invoke-Installer '9.9.9' $installDir | Out-String
    $installed = Join-Path $installDir 'televim.exe'
    Assert (Test-Path -LiteralPath $installed) 'Installer did not copy televim.exe.'
    Assert ($installOutput.Contains('Installed Televim')) 'Installer did not report success after checksum verification.'
    Assert ([Environment]::GetEnvironmentVariable('Path', 'User') -eq $userPathBefore) '-NoModifyPath changed the User PATH.'
    Invoke-Installer '9.9.9' $installDir | Out-Null
    Assert ((Get-ChildItem -LiteralPath $installDir -File).Count -eq 1) 'Repeated install left extra files.'

    $badDir = Join-Path $temporary 'bad-checksum-bin'
    Set-Content -LiteralPath (Join-Path $versionDir 'SHA256SUMS') -Value (('0' * 64) + "  $asset")
    Assert-InstallerFails '9.9.9' $badDir 'SHA-256 mismatch'
    Assert (-not (Test-Path -LiteralPath (Join-Path $badDir 'televim.exe'))) 'Checksum failure installed the binary.'

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
    Assert ($source.Contains("GetEnvironmentVariable('Path', 'User')") -and $source.Contains('SetEnvironmentVariable(''Path'', $newUserPath, ''User'')') -and $source.Contains('OrdinalIgnoreCase')) 'User PATH update must check for an existing entry.'
    Write-Output 'PowerShell installer tests passed.'
}
finally {
    if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue }
    if ($null -eq $oldArchitecture) { Remove-Item Env:PROCESSOR_ARCHITECTURE -ErrorAction SilentlyContinue }
    else { $env:PROCESSOR_ARCHITECTURE = $oldArchitecture }
    if ($null -eq $oldArchitectureW6432) { Remove-Item Env:PROCESSOR_ARCHITEW6432 -ErrorAction SilentlyContinue }
    else { $env:PROCESSOR_ARCHITEW6432 = $oldArchitectureW6432 }
    if ($null -eq $oldBaseUrl) { Remove-Item Env:TELEVIM_BASE_URL -ErrorAction SilentlyContinue }
    else { $env:TELEVIM_BASE_URL = $oldBaseUrl }
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
