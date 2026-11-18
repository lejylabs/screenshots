param(
    [Parameter(Mandatory=$true)][ValidateSet('Prepare','Preflight','Execute')][string]$Stage,
    [string]$FixtureDirectory,
    [string]$ShippingApplication,
    [string]$InstrumentedApplication,
    [string]$Controller,
    [uint32]$OwnerPid = 0
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_OS -cne 'Windows' -or $env:RUNNER_ENVIRONMENT -cne 'github-hosted') {throw 'Requires disposable GitHub-hosted Windows; never run on a user PC'}
$Identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]::new($Identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {throw 'Already elevated CI token required; no UAC or reboot is requested'}
$Sid = $Identity.User.Value
$RunnerTemp = (Resolve-Path -LiteralPath $env:RUNNER_TEMP).Path.TrimEnd('\')
$Drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($RunnerTemp))
if ($Drive.DriveType -ne [IO.DriveType]::Fixed) {throw 'Only a local fixed-volume sink is permitted'}
$Ancestor = [IO.DirectoryInfo]::new($RunnerTemp)
while ($Ancestor) {
    if (($Ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {throw 'No reparse points in the disposable temp root'}
    $Ancestor = $Ancestor.Parent
}
if ($Stage -eq 'Prepare') {
    if ($FixtureDirectory) {throw 'Prepare chooses its own fresh GUID directory'}
    $FixtureDirectory = Join-Path $RunnerTemp ('ScreenshotsPrintOwned-' + [guid]::NewGuid().ToString('D'))
    [IO.Directory]::CreateDirectory($FixtureDirectory) | Out-Null
    $Acl = [Security.AccessControl.DirectorySecurity]::new()
    $Acl.SetAccessRuleProtection($true,$false)
    $Acl.SetOwner($Identity.User)
    foreach ($Trustee in @($Sid,'S-1-5-18','S-1-5-32-544') | Select-Object -Unique) {
        $Acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($Trustee),'FullControl','ContainerInherit,ObjectInherit','None','Allow'))
    }
    Set-Acl -LiteralPath $FixtureDirectory -AclObject $Acl
    [pscustomobject]@{directory=$FixtureDirectory;queue=[IO.Path]::GetFileName($FixtureDirectory);port=(Join-Path $FixtureDirectory 'output.xps');owner_sid=$Sid} | ConvertTo-Json -Compress
    exit 0
}
if ([Threading.Thread]::CurrentThread.ApartmentState -ne 'STA') {throw 'Execute requires Windows PowerShell -STA for owned WPF rendering'}
$FixtureDirectory = (Resolve-Path -LiteralPath $FixtureDirectory).Path.TrimEnd('\')
if (-not $FixtureDirectory.StartsWith($RunnerTemp+'\',[StringComparison]::OrdinalIgnoreCase)) {throw 'Fixture must remain under this disposable RUNNER_TEMP'}
$Queue = [IO.Path]::GetFileName($FixtureDirectory)
if ($Queue -cnotmatch '^ScreenshotsPrintOwned-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$') {throw 'Exact GUID directory/queue required'}
$At = [IO.DirectoryInfo]::new($FixtureDirectory)
while ($At -and $At.FullName.Length -ge $RunnerTemp.Length) {
    if (($At.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {throw 'No reparse-point fixture ancestors'}
    $At = $At.Parent
}
$Acl = Get-Acl -LiteralPath $FixtureDirectory
if (-not $Acl.AreAccessRulesProtected -or $Acl.Owner -ne $Identity.Name) {throw 'Private owner and protected DACL required'}
$Allowed = @($Sid,'S-1-5-18','S-1-5-32-544')
foreach ($Rule in $Acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier])) {
    if ($Rule.AccessControlType -ne 'Allow' -or $Rule.IdentityReference.Value -notin $Allowed -or $Rule.IsInherited) {throw 'Unexpected private directory access rule'}
}
$Port = Join-Path $FixtureDirectory 'output.xps'
$InputImage = Join-Path $FixtureDirectory 'synthetic.png'
$ShippingApplication = (Resolve-Path -LiteralPath $ShippingApplication).Path
$InstrumentedApplication = (Resolve-Path -LiteralPath $InstrumentedApplication).Path
$Controller = (Resolve-Path -LiteralPath $Controller).Path
if (-not $OwnerPid) {throw 'Native supervisor ownership handoff required'}
$ThisProcess = Get-CimInstance -ClassName Win32_Process -Filter ('ProcessId = '+$PID)
$OwnerProcess = Get-CimInstance -ClassName Win32_Process -Filter ('ProcessId = '+$OwnerPid)
if ($ThisProcess.ParentProcessId -ne $OwnerPid -or -not $OwnerProcess -or $OwnerProcess.ExecutablePath -cne $Controller) {throw 'Preflight/Execute must be the exact native supervisor child'}
if ($ShippingApplication -eq $InstrumentedApplication) {throw 'Separate normal shipping and instrumented executables required'}
foreach ($Name in @('output.xps','synthetic.png','proof.json','oracle-control-golden.xps','oracle-control-shifted-fit.xps','oracle-control-wrong-color.xps','oracle-control-extra-page.xps','oracle-control-nested-brush-transform.xps','oracle-control-foreign-brush-namespace.xps','oracle-control-missing-width.xps','oracle-control-missing-height.xps')) {if (Test-Path -LiteralPath (Join-Path $FixtureDirectory $Name)) {throw 'Fresh reserved fixture outputs required'}}
Import-Module PrintManagement
$Driver = @(Get-PrinterDriver -Name 'Microsoft XPS Document Writer v4' -ErrorAction SilentlyContinue)
if ($Driver.Count -ne 1 -or $Driver[0].MajorVersion -ne 4) {throw 'PREREQUISITE MISSING: installed Microsoft XPS Document Writer v4; no download/install/feature/reboot fallback'}
$Signature = Get-AuthenticodeSignature -LiteralPath $Driver[0].DriverPath
if ($Signature.Status -ne 'Valid' -or -not $Signature.SignerCertificate -or $Signature.SignerCertificate.Subject -notmatch 'Microsoft') {throw 'Builtin driver binary must have a valid Microsoft signature'}
if ((Get-Service -Name Spooler).Status -ne 'Running') {throw 'PREREQUISITE MISSING: running spool service; never start/restart global spooler'}
if ($Stage -eq 'Preflight' -and (@(Get-Printer -Name $Queue -ErrorAction SilentlyContinue).Count -or @(Get-PrinterPort -Name $Port -ErrorAction SilentlyContinue).Count)) {throw 'GUID collision: refuse existing queue/port; no creation authority acquired'}
Add-Type -AssemblyName WindowsBase,PresentationCore,PresentationFramework,ReachFramework,System.IO.Compression
if ($Stage -eq 'Preflight') {exit 0}
. (Join-Path $PSScriptRoot 'xps-oracle.ps1')
$Proof = [ordered]@{instrumented_downstream_only=$true;normal_policy_refused=$false;oracle_controls=$null;driver=$Driver[0].Name;driver_sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $Driver[0].DriverPath).Hash;cases=@();passed=$false}

function Assert-OwnedQueue {
    $Printer = Get-Printer -Name $Queue -Full
    $Ports = @(Get-PrinterPort -Name $Port)
    if ($Printer.DriverName -cne 'Microsoft XPS Document Writer v4' -or $Printer.PortName -cne $Port -or $Printer.Shared -or $Printer.Type -ne 'Local' -or $Ports.Count -ne 1 -or $Ports[0].PortMonitor -ne 'Local Port') {throw 'Owned queue/driver/nonforwarding Local Port identity changed'}
    $Security = [Security.AccessControl.RawSecurityDescriptor]::new($Printer.PermissionSDDL)
    if ($Security.Owner.Value -ne $Sid -or -not ($Security.ControlFlags -band [Security.AccessControl.ControlFlags]::DiscretionaryAclProtected)) {throw 'Owned queue owner/protected DACL changed'}
    foreach ($Ace in $Security.DiscretionaryAcl) {
        if ($Ace.AceQualifier -ne [Security.AccessControl.AceQualifier]::AccessAllowed -or $Ace.SecurityIdentifier.Value -notin $Allowed) {throw 'Unexpected principal on private owned queue'}
    }
}
function Pause-OwnedQueue {
    Assert-OwnedQueue
    $Print = Get-CimInstance -ClassName Win32_Printer -Filter ("Name = '"+$Queue+"'")
    if ((Invoke-CimMethod -InputObject $Print -MethodName Pause).ReturnValue -ne 0) {throw 'Owned queue pause failed'}
    if (@(Get-PrintJob -PrinterName $Queue).Count) {throw 'Owned queue is not empty before case'}
}
function Resume-OwnedQueue {
    Assert-OwnedQueue
    $Print = Get-CimInstance -ClassName Win32_Printer -Filter ("Name = '"+$Queue+"'")
    if ((Invoke-CimMethod -InputObject $Print -MethodName Resume).ReturnValue -ne 0) {throw 'Owned queue resume failed'}
}
function Invoke-OwnedCli([string]$Case,[string]$Application,[string]$Mode='normal',[string]$Paper='letter') {
    Pause-OwnedQueue
    $Stdout = Join-Path $FixtureDirectory ($Case+'.stdout.log')
    $Stderr = Join-Path $FixtureDirectory ($Case+'.stderr.log')
    $ControllerJson = & $Controller invoke $Application $InputImage $Queue $Paper $Stdout $Stderr $Mode
    if ($LASTEXITCODE -ne 0) {throw ('Native owned launch failed: '+$Case)}
    $Launch = $ControllerJson | ConvertFrom-Json
    foreach ($Log in @($Stdout,$Stderr)) {if ((Get-Item -LiteralPath $Log).Length -gt 4096) {throw 'Unexpected/oversized public diagnostic'}}
    $Out = [IO.File]::ReadAllText($Stdout).Trim()
    $Err = [IO.File]::ReadAllText($Stderr)
    $Jobs = @(Get-PrintJob -PrinterName $Queue)
    return [pscustomobject]@{case=$Case;exit=$Launch.application_exit;stdout=$Out;stderr=$Err;jobs=$Jobs}
}
function Assert-OneOwnedJob($Case) {
    if ($Case.jobs.Count -ne 1 -or $Case.jobs[0].DocumentName -cne 'Screenshot') {throw 'Expected exactly one owned spool job and fixed title; no duplicate/retry allowed'}
}
function Assert-Acceptance($Case) {
    if ($Case.exit -ne 0 -or $Case.stderr) {throw 'Native submission did not succeed'}
    $Json = $Case.stdout | ConvertFrom-Json
    if (@($Json.PSObject.Properties).Count -ne 4 -or $Json.status -cne 'accepted_by_spooler' -or $Json.physical_delivery -cne 'unverified' -or $Json.copies -ne 1 -or $Json.pages -ne 1) {throw 'Wrong public print acknowledgement'}
    Assert-OneOwnedJob $Case
}
function Complete-OwnedOutput([string]$Case,[string]$Paper='letter') {
    Resume-OwnedQueue
    $Until = [DateTime]::UtcNow.AddSeconds(30)
    $Ready = $false
    while ([DateTime]::UtcNow -lt $Until) {
        Assert-OwnedQueue
        if (@(Get-PrintJob -PrinterName $Queue).Count -eq 0 -and (Test-Path -LiteralPath $Port)) {
            $Read = $null
            try {
                $Read = [IO.File]::Open($Port,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::None)
                if ($Read.Length -gt 0 -and $Read.Length -le 16777216) {$Ready=$true}
            } catch [IO.IOException] {} finally {if ($Read) {$Read.Dispose()}}
            if ($Ready) {break}
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $Ready) {throw 'Owned spool output did not become complete, bounded and exclusively readable'}
    $Oracle = Test-OwnedXpsOutput $Port $Paper
    [IO.File]::Delete($Port)
    return $Oracle
}

# The outer native supervisor always owns final spool/port cleanup, including
# exceptions, process termination and power-user cmdlet failure.
try {
    # Only the outer native supervisor creates resources and retains their
    # returned authority. Execute may validate/use them, never adopt a collision.
    Assert-OwnedQueue
    New-OwnedPrintImage $InputImage
    $SourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $InputImage).Hash
    $Proof.oracle_controls = Test-OwnedOracleControls $FixtureDirectory $InputImage
    $Normal = Invoke-OwnedCli 'normal-policy' $ShippingApplication
    if ($Normal.exit -eq 0 -or $Normal.stdout -or $Normal.stderr -notmatch 'before submission' -or $Normal.jobs.Count -ne 0 -or (Test-Path -LiteralPath $Port)) {throw 'Normal shipping policy failed to refuse the same owned file-port queue'}
    $Proof.normal_policy_refused = $true
    $Healthy = Invoke-OwnedCli 'native-output' $InstrumentedApplication
    Assert-Acceptance $Healthy
    $Pixels = Complete-OwnedOutput 'native-output'
    $Proof.cases += [ordered]@{case='native-output';spool_jobs=1;accepted=$true;oracle=$Pixels}
    $A4 = Invoke-OwnedCli 'native-output-a4' $InstrumentedApplication 'normal' 'a4'
    Assert-Acceptance $A4
    $Pixels = Complete-OwnedOutput 'native-output-a4' 'a4'
    $Proof.cases += [ordered]@{case='native-output-a4';spool_jobs=1;accepted=$true;oracle=$Pixels}
    $Broken = Invoke-OwnedCli 'broken-stdout' $InstrumentedApplication 'broken-stdout'
    if ($Broken.exit -eq 0 -or $Broken.stdout -or $Broken.stderr -notmatch 'accepted.*result output failed|accepted.*output failed' -or $Broken.stderr -notmatch 'do not retry automatically') {throw 'Accepted-but-broken-stdout status lost ambiguity or retry warning'}
    Assert-OneOwnedJob $Broken
    $Pixels = Complete-OwnedOutput 'broken-stdout'
    $Proof.cases += [ordered]@{case='broken-stdout';spool_jobs=1;result_failed_after_acceptance=$true;oracle=$Pixels}
    # Real Local Port failure: deny the spool service any writable sharing of
    # this exact owned output file, while leaving all real queues untouched.
    $Block = [IO.File]::Open($Port,[IO.FileMode]::CreateNew,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    try {
        $Blocked = Invoke-OwnedCli 'native-port-denied' $InstrumentedApplication
        Assert-Acceptance $Blocked
        Resume-OwnedQueue
        $Until = [DateTime]::UtcNow.AddSeconds(15)
        $Held = $null
        while ([DateTime]::UtcNow -lt $Until) {
            $Jobs = @(Get-PrintJob -PrinterName $Queue)
            if ($Jobs.Count -gt 1) {throw 'Native port fault caused duplicate submission'}
            if ($Jobs.Count -eq 1 -and $Jobs[0].ID -eq $Blocked.jobs[0].ID -and [string]$Jobs[0].JobStatus -match 'Error|Blocked|Offline') {$Held=$Jobs[0];break}
            Start-Sleep -Milliseconds 100
        }
        if (-not $Held -or $Block.Length -ne 0) {throw 'Native port sharing fault was not independently observed; no fake success'}
        Remove-PrintJob -PrinterName $Queue -ID $Held.ID
        $Proof.cases += [ordered]@{case='native-port-denied';spool_jobs=1;accepted_delivery_unverified=$true;native_status=[string]$Held.JobStatus;no_output=$true}
    } finally {$Block.Dispose()}
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $InputImage).Hash -cne $SourceHash) {throw 'Source image changed'}
    $Proof.passed = $true
} finally {
    # Proof remains under the retained private ACL even when downstream execution fails.
    $Proof | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $FixtureDirectory 'proof.json') -Encoding UTF8
}
if (-not $Proof.passed) {throw 'Owned native spool fixture failed'}
$Proof | ConvertTo-Json -Depth 8 -Compress
