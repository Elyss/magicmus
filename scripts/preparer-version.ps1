# Prépare une version publiable de MagicMus : compilation, copie dans dist\ et empreinte
# SHA-256 (vérifiée par la mise à jour automatique avant d'installer).
#
#   powershell -ExecutionPolicy Bypass -File scripts\preparer-version.ps1
#
# La version est celle de Cargo.toml : l'augmenter avant chaque publication.
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
Write-Host "Version $version"

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "échec de la compilation" }
cargo test --release -q
if ($LASTEXITCODE -ne 0) { throw "échec des tests" }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\karaoke.exe dist\MagicMus.exe -Force
$sha = (Get-FileHash dist\MagicMus.exe -Algorithm SHA256).Hash.ToLower()
[IO.File]::WriteAllText("$PWD\dist\MagicMus.exe.sha256", "$sha  MagicMus.exe`n", (New-Object Text.UTF8Encoding $false))

Write-Host ""
Write-Host "Prêt : dist\MagicMus.exe ($sha)"
Write-Host "Ensuite :"
Write-Host "  git tag v$version ; git push ; git push origin v$version"
Write-Host "  GitHub > Releases > Draft a new release > étiquette v$version,"
Write-Host "  joindre dist\MagicMus.exe et dist\MagicMus.exe.sha256, puis Publish."
Write-Host "  (ou : gh release create v$version dist\MagicMus.exe dist\MagicMus.exe.sha256 --title `"MagicMus $version`")"
