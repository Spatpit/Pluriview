[CmdletBinding()]
param(
    [string]$Sdk = "$env:LOCALAPPDATA\Android\Sdk",
    [string]$BuildTools = '35.0.0',
    [string]$Platform = 'android-35'
)
$ErrorActionPreference = 'Stop'
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$source = Join-Path $workspace 'android'
$output = Join-Path $workspace 'target\android-viewer'
$tools = Join-Path $Sdk "build-tools\$BuildTools"
$androidJar = Join-Path $Sdk "platforms\$Platform\android.jar"
foreach ($path in @($androidJar, "$tools\aapt2.exe", "$tools\d8.bat", "$tools\zipalign.exe", "$tools\apksigner.bat")) {
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing Android SDK tool: $path" }
}
foreach ($name in @('java', 'javac', 'jar', 'keytool')) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) { throw "Install a JDK and put $name on PATH." }
}
function Invoke-Checked {
    param([string]$Tool, [string[]]$Arguments)
    & $Tool @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Tool failed with exit code $LASTEXITCODE" }
}
# Unique build directories prevent stale class files from entering the APK.
$build = Join-Path $output ([Guid]::NewGuid().ToString('N'))
$classes = Join-Path $build 'classes'
$dex = Join-Path $build 'dex'
$tests = Join-Path $build 'tests'
$resources = Join-Path $build 'res'
$drawable = Join-Path $resources 'drawable'
$assets = Join-Path $build 'assets'
New-Item -ItemType Directory -Force -Path $classes, $dex, $tests, $drawable, $assets | Out-Null
Copy-Item -LiteralPath (Join-Path $workspace 'assets\icon.png') -Destination (Join-Path $drawable 'pluriview.png')
Copy-Item -LiteralPath (Join-Path $workspace 'LICENSE') -Destination $assets
$resourceZip = Join-Path $build 'resources.zip'
Invoke-Checked "$tools\aapt2.exe" @('compile', '--dir', $resources, '-o', $resourceZip)
$javaSources = @(Get-ChildItem -LiteralPath (Join-Path $source 'src\io\pluriview\viewer') -Filter '*.java' | ForEach-Object FullName)
Invoke-Checked javac (@('-encoding', 'UTF-8', '--release', '8', '-classpath', $androidJar, '-d', $classes) + $javaSources)
Invoke-Checked javac @('-encoding', 'UTF-8', '--release', '8', '-classpath', $classes, '-d', $tests, "$source\tests\CameraTransformTest.java", "$source\tests\VideoPacketTest.java", "$source\tests\DisplayMappingTest.java", "$source\tests\DoubleTapTest.java", "$source\tests\StreamTimingTest.java", "$source\tests\DecoderLifecycleTest.java")
Invoke-Checked javac @('-encoding', 'UTF-8', '--release', '8', '-classpath', "$classes;$tests", '-d', $tests, "$source\tests\VideoConnectionTest.java")
Invoke-Checked java @('-cp', "$classes;$tests", 'io.pluriview.viewer.VideoConnectionTest')
Invoke-Checked java @('-cp', "$classes;$tests", 'CameraTransformTest')
Invoke-Checked java @('-cp', "$classes;$tests", 'io.pluriview.viewer.VideoPacketTest')
Invoke-Checked java @('-cp', "$classes;$tests", 'DisplayMappingTest')
Invoke-Checked java @('-cp', "$classes;$tests", 'DoubleTapTest')
Invoke-Checked java @('-cp', "$classes;$tests", 'io.pluriview.viewer.StreamTimingTest')
Invoke-Checked java @('-cp', "$classes;$tests", 'io.pluriview.viewer.DecoderLifecycleTest')
$classJar = Join-Path $build 'classes.jar'
Invoke-Checked jar @('cf', $classJar, '-C', $classes, '.')
Invoke-Checked "$tools\d8.bat" @('--lib', $androidJar, '--min-api', '26', '--output', $dex, $classJar)
$unsigned = Join-Path $build 'unsigned.apk'
Invoke-Checked "$tools\aapt2.exe" @('link', '-I', $androidJar, '--manifest', "$source\AndroidManifest.xml", '-A', $assets, '-o', $unsigned, $resourceZip)
Invoke-Checked jar @('uf', $unsigned, '-C', $dex, 'classes.dex')
$aligned = Join-Path $build 'aligned.apk'
Invoke-Checked "$tools\zipalign.exe" @('-f', '4', $unsigned, $aligned)
# Local test signing only. Neither this key nor generated APKs enter Git.
$key = Join-Path $output 'debug.keystore'
if (-not (Test-Path -LiteralPath $key)) {
    Invoke-Checked keytool @('-genkeypair', '-keystore', $key, '-storepass', 'android', '-keypass', 'android', '-alias', 'androiddebugkey', '-keyalg', 'RSA', '-keysize', '2048', '-validity', '10000', '-dname', 'CN=Android Debug,O=Android,C=US')
}
$apk = Join-Path $build 'pluriview-viewer-debug.apk'
Invoke-Checked "$tools\apksigner.bat" @('sign', '--ks', $key, '--ks-pass', 'pass:android', '--key-pass', 'pass:android', '--out', $apk, $aligned)
Invoke-Checked "$tools\apksigner.bat" @('verify', '--verbose', $apk)
$destination = Join-Path $output 'pluriview-viewer-debug.apk'
Copy-Item -LiteralPath $apk -Destination $destination -Force
Write-Output "Android test APK: $destination"
