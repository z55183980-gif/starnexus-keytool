param(
    [Parameter(Mandatory=$true)][ValidateSet('claude', 'codex')][string]$Product,
    [Parameter(Mandatory=$true)][string]$PackagePath,
    [string]$LicensePath,
    [string]$ResultPath,
    [switch]$VerifyOnly
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
$packageStream = $null
$licenseStream = $null
$exitCode = 1
try {
    # Keep the verified files locked against replacement until Windows finishes reading them.
    $packageStream = [IO.File]::Open($PackagePath, 'Open', 'Read', 'Read')
    $signature = Get-AuthenticodeSignature -LiteralPath $PackagePath
    if ($signature.Status -ne 'Valid') { throw 'Package signature verification failed.' }
    Add-Type -AssemblyName System.IO.Compression
    $archive = [IO.Compression.ZipArchive]::new($packageStream, [IO.Compression.ZipArchiveMode]::Read, $true)
    try {
        $entry = $archive.GetEntry('AppxManifest.xml')
        if (-not $entry -or $entry.Length -gt 1048576) { throw 'Invalid package manifest.' }
        $settings = [Xml.XmlReaderSettings]::new()
        $settings.DtdProcessing = [Xml.DtdProcessing]::Prohibit
        $settings.XmlResolver = $null
        $reader = [Xml.XmlReader]::Create($entry.Open(), $settings)
        try { $manifest = [Xml.XmlDocument]::new(); $manifest.XmlResolver = $null; $manifest.Load($reader) }
        finally { $reader.Dispose() }
    } finally { $archive.Dispose() }
    $identity = $manifest.Package.Identity
    if ($Product -eq 'codex') {
        if ($identity.Name -cne 'OpenAI.Codex' -or $identity.Publisher -cne 'CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B') {
            throw 'The package is not the official Codex desktop app.'
        }
    } elseif ($identity.Name -cne 'Claude' -or $identity.Publisher -notmatch 'Anthropic, PBC' -or $signature.SignerCertificate.Subject -notmatch 'Anthropic, PBC') {
        throw 'The package is not the official Claude desktop app.'
    }
    $architecture = $env:PROCESSOR_ARCHITEW6432
    if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
    $expectedArch = if ($architecture -eq 'ARM64') { 'arm64' } else { 'x64' }
    if ($identity.ProcessorArchitecture -ne $expectedArch) { throw 'Package architecture does not match this computer.' }
    $os = [version](Get-CimInstance Win32_OperatingSystem).Version
    foreach ($family in $manifest.Package.Dependencies.TargetDeviceFamily) {
        if ($family.Name -in @('Windows.Desktop', 'Windows.Universal') -and $os -lt [version]$family.MinVersion) {
            throw ('Windows ' + $family.MinVersion + ' or newer is required.')
        }
    }
    if ($Product -eq 'codex') {
        if (-not $LicensePath) { throw 'The offline license is missing.' }
        $licenseStream = [IO.File]::Open($LicensePath, 'Open', 'Read', 'Read')
        if ($licenseStream.Length -eq 0 -or $licenseStream.Length -gt 1048576) { throw 'Invalid offline license.' }
    }
    if ($VerifyOnly) {
        @{ name=$identity.Name; version=$identity.Version; architecture=$identity.ProcessorArchitecture; signature=[string]$signature.Status; signer=$signature.SignerCertificate.Subject; dependencies=$manifest.Package.Dependencies.OuterXml } | ConvertTo-Json -Compress
        $exitCode = 0
    } else {
        $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
        if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Administrator approval is required.' }
        # Another installer or the app's own updater may have finished while downloading.
        $registered = @(Get-AppxPackage -AllUsers -Name $identity.Name -ErrorAction Stop)
        $provisioned = @(Get-AppxProvisionedPackage -Online -ErrorAction Stop | Where-Object DisplayName -eq $identity.Name)
        foreach ($existing in @($registered) + @($provisioned)) {
            if ([version]$existing.Version -ge [version]$identity.Version) { throw 'This version or a newer version is already installed. Sign back into Windows if registration is pending.' }
        }
        if ($Product -eq 'codex') {
            Add-AppxProvisionedPackage -Online -PackagePath $PackagePath -LicensePath $LicensePath -Regions all -ErrorAction Stop | Out-Null
        } else {
            Add-AppxProvisionedPackage -Online -PackagePath $PackagePath -SkipLicense -Regions all -ErrorAction Stop | Out-Null
        }
        if ($ResultPath) { @{ success=$true } | ConvertTo-Json -Compress | Set-Content -LiteralPath $ResultPath -Encoding UTF8 }
        $exitCode = 0
    }
} catch {
    if ($ResultPath -and -not $VerifyOnly) { @{ success=$false; error=$_.Exception.Message } | ConvertTo-Json -Compress | Set-Content -LiteralPath $ResultPath -Encoding UTF8 }
    [Console]::Error.WriteLine($_.Exception.Message)
} finally {
    if ($licenseStream) { $licenseStream.Dispose() }
    if ($packageStream) { $packageStream.Dispose() }
}
exit $exitCode
