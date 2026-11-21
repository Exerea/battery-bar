$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$userProfile = $env:USERPROFILE
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $userProfile ".cargo" }

# Remap local paths dynamically without hardcoding any username in repo files.
$flags = @(
    "--remap-path-prefix", "$root=/src",
    "--remap-path-prefix", "$cargoHome=/cargo",
    "--remap-path-prefix", "$userProfile=/user",
    "-C", "link-arg=/PDBALTPATH:none",
    "-C", "link-arg=/BREPRO"
)
$env:CARGO_ENCODED_RUSTFLAGS = ($flags -join [char]0x1F)

cargo build --release --manifest-path (Join-Path $root "Cargo.toml")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$exe = Join-Path $root "target\release\battery-bar.exe"
$item = Get-Item $exe
Write-Output ("Built clean release binary: {0} ({1:N1} KB)" -f $exe, ($item.Length / 1024))
