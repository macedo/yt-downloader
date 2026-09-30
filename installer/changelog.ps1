# Gera as notas de um Release a partir dos commits desde a tag anterior.
# Uso: installer\changelog.ps1 -Tag v0.5.0 [-Repo macedo/yt-downloader]
param(
    [Parameter(Mandatory)] [string] $Tag,
    [string] $Repo = 'macedo/yt-downloader'
)

# Commits com acentos: o git escreve em UTF-8.
[Console]::OutputEncoding = [Text.Encoding]::UTF8

$previous = git describe --tags --abbrev=0 "$Tag^" 2>$null
$range = if ($previous) { "$previous..$Tag" } else { $Tag }

# Commits cujo título é só a versão ("v0.5.0") apenas mudam o número; ficam de fora.
$changes = @(git log $range --no-merges --reverse --format='- %s (%h)' |
    Where-Object { $_ -notmatch '^- v\d+\.\d+\.\d+ \(' })
if ($LASTEXITCODE -ne 0) { throw "git log falhou para $range" }
if ($changes.Count -eq 0) { $changes = @('- Sem mudanças além da versão.') }

$version = $Tag.TrimStart('v')
$notes = @('## Mudanças', '') + $changes
if ($previous) {
    $notes += @('', "**Comparação completa:** https://github.com/$Repo/compare/$previous...$Tag")
}
$notes += @(
    '',
    '## Instalação',
    '',
    "Baixe o **YT-Downloader-Setup-$version.exe** abaixo e execute. Para atualizar, basta instalar por cima da versão anterior.",
    '',
    'O instalador não tem assinatura digital: se o Windows SmartScreen avisar, clique em **Mais informações → Executar assim mesmo**.'
)
$notes -join "`n"
