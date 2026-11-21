# Parse text as data. Never invoke, expand, dot-source, or compile the input.
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$source = [Console]::In.ReadToEnd()
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errors)
$complete = @($errors).Count -eq 0
$commands = New-Object 'System.Collections.Generic.List[object]'
$accesses = New-Object 'System.Collections.Generic.List[object]'
$redirects = New-Object 'System.Collections.Generic.List[object]'
$commandIndexes = @{}
$allowed = @('ScriptBlockAst','NamedBlockAst','PipelineAst','PipelineChainAst','CommandAst','StringConstantExpressionAst','ConstantExpressionAst','CommandParameterAst','FileRedirectionAst','MergingRedirectionAst')
$nodes = $ast.FindAll({param($node) $true}, $true)
if ($nodes.Count -gt 4096 -or @($tokens).Count -gt 32768) { $complete = $false }
foreach ($node in $nodes | Microsoft.PowerShell.Utility\Select-Object -First 4096) {
    $kind = $node.GetType().Name
    if ($allowed -notcontains $kind) { $complete = $false }
    if ($kind -eq 'PipelineAst' -and $node.Background) { $complete = $false }
    if ($kind -eq 'CommandAst') {
        if ($node.InvocationOperator.ToString() -ne 'Unknown') { $complete = $false }
        $argv = New-Object 'System.Collections.Generic.List[string]'
        foreach ($element in $node.CommandElements) {
            switch ($element.GetType().Name) {
                'StringConstantExpressionAst' { $argv.Add([string]$element.Value) }
                'ConstantExpressionAst' { $argv.Add([string]$element.Value) }
                'CommandParameterAst' {
                    $argv.Add('-' + $element.ParameterName)
                    if ($null -ne $element.Argument) {
                        if ($element.Argument.GetType().Name -in @('StringConstantExpressionAst','ConstantExpressionAst')) { $argv.Add([string]$element.Argument.Value) }
                        else { $complete = $false }
                    }
                }
                default { $complete = $false }
            }
        }
        $commands.Add(@($argv.ToArray()))
        $commandIndexes[$node.Extent.StartOffset] = $commands.Count - 1
        $files = New-Object 'System.Collections.Generic.List[string]'
        foreach ($redirect in $node.Redirections) {
            if ($redirect.GetType().Name -eq 'FileRedirectionAst' -and $redirect.Location.GetType().Name -eq 'StringConstantExpressionAst') {
                $files.Add([string]$redirect.Location.Value)
            }
        }
        $redirects.Add(@($files.ToArray()))
    }
    if ($kind -eq 'FileRedirectionAst') {
        if ($node.Location.GetType().Name -ne 'StringConstantExpressionAst') { $complete = $false }
    }
}
function Convert-PermissionTree($node, [int]$depth) {
    if ($null -eq $node -or $depth -gt 32) { return @{kind='sequence';value=@()} }
    switch ($node.GetType().Name) {
        'ScriptBlockAst' { return Convert-PermissionTree $node.EndBlock ($depth + 1) }
        'NamedBlockAst' {
            $children = @($node.Statements | ForEach-Object { Convert-PermissionTree $_ ($depth + 1) })
            return @{kind='sequence';value=$children}
        }
        'PipelineAst' {
            $children = @($node.PipelineElements | ForEach-Object { Convert-PermissionTree $_ ($depth + 1) })
            return @{kind='pipe';value=$children}
        }
        'PipelineChainAst' {
            $kind = if ($node.Operator.ToString() -eq 'AndAnd') { 'and' } else { 'or' }
            return @{kind=$kind;value=@((Convert-PermissionTree $node.LhsPipelineChain ($depth + 1)), (Convert-PermissionTree $node.RhsPipeline ($depth + 1)))}
        }
        'CommandAst' { return @{kind='command';value=$commandIndexes[$node.Extent.StartOffset]} }
        default { return @{kind='sequence';value=@()} }
    }
}
$tree = Convert-PermissionTree $ast 0
@{complete=$complete;commands=@($commands.ToArray());accesses=@();redirects=@($redirects.ToArray());tree=$tree} | Microsoft.PowerShell.Utility\ConvertTo-Json -Compress -Depth 100
