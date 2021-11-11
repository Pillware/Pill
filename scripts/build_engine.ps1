$location = Get-Location

Write-Host "Building core"
Set-Location -Path $location"\pill\src\core\"
cargo build

Write-Host "Building engine"
Set-Location -Path $location"\pill\src\engine\"
cargo build

Write-Host "Building renderer"
Set-Location -Path $location"\pill\src\graphics\"
cargo build


Set-Location -Path $location
Write-Host "Done"