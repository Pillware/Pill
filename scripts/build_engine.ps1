$location = Get-Location

Write-Host "Building core"
Set-Location -Path $location"\crates\pill_core\"
cargo build

Write-Host "Building engine"
Set-Location -Path $location"\crates\pill_engine\"
cargo build

Write-Host "Building renderer"
Set-Location -Path $location"\crates\pill_renderer\"
cargo build


Set-Location -Path $location
Write-Host "Done"