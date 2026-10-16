# Converts the C# `Strings/Strings.*.xaml` ResourceDictionaries into flat JSON
# catalogs for the Rust port (`locales/<lang>.json`).
#
# Encoding rules (per repo AGENTS.md):
# - Read XML with its own encoding declaration (UTF-8), write JSON as UTF-8 (no BOM).
# - XML entities (&amp; &lt; ...) decode at load time in WPF, so JSON stores decoded text.
# - Verification is done with code-point escapes, never raw terminal output.
param(
    [string]$Source = (Join-Path $PSScriptRoot "..\..\orbital-main\Strings"),
    [string]$Dest   = (Join-Path $PSScriptRoot "..\locales")
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Path $Dest -Force | Out-Null

foreach ($file in Get-ChildItem -Path $Source -Filter "Strings.*.xaml" | Sort-Object Name) {
    $lang = $file.BaseName -replace '^Strings\.', ''
    $doc = New-Object System.Xml.XmlDocument
    $doc.Load($file.FullName)   # honors the XML encoding declaration

    $map = [ordered]@{}
    foreach ($node in $doc.SelectNodes("//*[local-name()='String']")) {
        $key = $node.GetAttribute("Key", "http://schemas.microsoft.com/winfx/2006/xaml")
        if ([string]::IsNullOrWhiteSpace($key)) { continue }
        $map[$key] = $node.InnerText   # entities decoded, newlines preserved
    }

    # Manual JSON emission keeps full control over escaping (serde_json-compatible).
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.Append("{`n")
    $i = 0
    foreach ($k in $map.Keys) {
        $v = $map[$k]
        $ke = $k.Replace('\', '\\').Replace('"', '\"')
        $ve = $v.Replace('\', '\\').Replace('"', '\"').Replace("`r", '\r').Replace("`n", '\n').Replace("`t", '\t')
        [void]$sb.Append("  ""$ke"": ""$ve""")
        if (++$i -lt $map.Count) { [void]$sb.Append(",") }
        [void]$sb.Append("`n")
    }
    [void]$sb.Append("}")

    $outPath = Join-Path $Dest "$lang.json"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($outPath, $sb.ToString(), $utf8NoBom)
    Write-Output "$lang -> $lang.json ($($map.Count) keys)"
}
