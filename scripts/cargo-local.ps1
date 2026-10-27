$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$localCargo = Join-Path $projectRoot '.tools\cargo\bin\cargo.exe'

if (-not (Test-Path -LiteralPath $localCargo)) {
    throw 'Local Rust toolchain not found under .tools. Use your installed cargo, or provision a local toolchain first.'
}

$env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
$env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
$compilerDirectory = Get-ChildItem -LiteralPath (Join-Path $projectRoot '.tools') -Directory -Filter 'llvm-mingw-*-msvcrt-x86_64' |
    Sort-Object Name -Descending |
    Select-Object -First 1

if ($compilerDirectory) {
    $compilerBin = Join-Path $compilerDirectory.FullName 'bin'
    $env:PATH = "$compilerBin;$env:PATH"
    $env:CC_x86_64_pc_windows_gnu = Join-Path $compilerBin 'x86_64-w64-mingw32-gcc.exe'
    $env:RUSTFLAGS = "$env:RUSTFLAGS -Clink-self-contained=yes".Trim()
}

Push-Location -LiteralPath $projectRoot
try {
    & $localCargo @args
    $cargoExitCode = $LASTEXITCODE
}
finally {
    Pop-Location
}
exit $cargoExitCode
