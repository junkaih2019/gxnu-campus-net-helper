# Windows Cargo wrapper. Examples:
#   .\build.ps1 verify
#   .\build.ps1 test --locked --offline -- --list
#   .\build.ps1 run -- --once
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

# PowerShell consumes the first bare -- when its target is a script. The
# invocation AST distinguishes it from a quoted, escaped or splatted literal.
# Use the already bound arguments for all values; never re-evaluate expressions.
$cargoArgs = [Collections.Generic.List[object]]::new()
$cargoArgs.AddRange([object[]]$args)
if ($MyInvocation.Line) {
    # Windows PowerShell 5.1 has no InvocationInfo.Statement. The caller's
    # position includes continuation lines and identifies this call in a pipeline.
    $position = (Get-PSCallStack)[1].Position
    $ast = [Management.Automation.Language.Parser]::ParseInput(
        $position.Text, [ref]$null, [ref]$null)
    $line = $MyInvocation.ScriptLineNumber - $position.StartLineNumber + 1
    $column = $MyInvocation.OffsetInLine
    if ($line -eq 1) { $column -= $position.StartColumnNumber - 1 }
    $call = $ast.Find({
        param($node)
        $node -is [Management.Automation.Language.CommandAst] -and
            $node.Extent.StartLineNumber -eq $line -and $node.Extent.StartColumnNumber -eq $column
    }, $true)
    $elements = $call.CommandElements
    for ($i = 1; $i -lt $elements.Count; $i++) {
        $element = $elements[$i]
        if ($element -is [Management.Automation.Language.CommandParameterAst] -and $element.Extent.Text -ceq '--') {
            $tailCount = 0
            foreach ($tail in ($elements | Select-Object -Skip ($i + 1))) {
                if ($tail -is [Management.Automation.Language.VariableExpressionAst] -and $tail.Splatted) {
                    $variable = Get-Variable -Name $tail.VariablePath.UserPath -Scope 1 -ErrorAction SilentlyContinue
                    if ($null -ne $variable.Value) { $tailCount += @($variable.Value).Count }
                } else {
                    $tailCount++
                }
            }
            $cargoArgs.Insert($cargoArgs.Count - $tailCount, '--')
            break
        }
    }
}
if ($cargoArgs.Count -eq 0) { $cargoArgs.Add('build') }

$mingwBin = if ($env:MINGW_BIN) { $env:MINGW_BIN } else { Join-Path $env:USERPROFILE 'tools\mingw64\bin' }
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$previousPath = $env:PATH
Push-Location $PSScriptRoot
try {
    $pathParts = @($cargoBin, $previousPath)
    if (Test-Path -LiteralPath $mingwBin) { $pathParts = @($mingwBin) + $pathParts }
    $env:PATH = $pathParts -join ';'

    if ($cargoArgs[0] -eq 'verify') {
        if ($cargoArgs.Count -ne 1) { throw 'Usage: .\build.ps1 verify' }
        $steps = @(
            @('fmt', '--all', '--', '--check'),
            @('clippy', '--locked', '--all-targets', '--', '-D', 'warnings'),
            @('test', '--locked'),
            @('build', '--locked', '--release')
        )
        foreach ($step in $steps) {
            Write-Host "cargo $($step -join ' ')"
            & cargo @step
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        }
        exit 0
    }

    $forward = $cargoArgs.ToArray()
    & cargo @forward
    exit $LASTEXITCODE
} finally {
    $env:PATH = $previousPath
    Pop-Location
}
