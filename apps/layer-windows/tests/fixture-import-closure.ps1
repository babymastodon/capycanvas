param([string]$Scripts=(Join-Path $PSScriptRoot '../scripts'),[switch]$SelfTestOnly)
$ErrorActionPreference='Stop'
$script:closureAsts=@{}
$script:closureExternal=@{}
$script:closureProviders=@{}
function Read-ClosureAst($Path){
 $Path=[IO.Path]::GetFullPath($Path)
 if(!$script:closureAsts.ContainsKey($Path)){
  $tokens=$null;$errors=$null;$tree=[Management.Automation.Language.Parser]::ParseFile($Path,[ref]$tokens,[ref]$errors)
  if($errors.Count){throw ($Path+': '+($errors.Message -join '; '))};$script:closureAsts[$Path]=$tree
 }
 $script:closureAsts[$Path]
}
function Closure-Scope($Node){
 for($at=$Node;$at;$at=$at.Parent){
  if($at -is [Management.Automation.Language.ScriptBlockAst] -and (!$at.Parent -or $at.Parent -is [Management.Automation.Language.FunctionDefinitionAst])){return $at}
  if($at -is [Management.Automation.Language.ScriptBlockAst] -and $at.Parent -is [Management.Automation.Language.ScriptBlockExpressionAst]){
   $command=$at.Parent.Parent
   if($command -is [Management.Automation.Language.CommandAst] -and $command.InvocationOperator -eq 'Ampersand'){return $at}
  }
 }
}
function Closure-SameScope($Node,$Scope){[object]::ReferenceEquals((Closure-Scope $Node),$Scope)}
function Closure-Path($Node,$Directory){
 $join=$Node.Find({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Join-Path'},$true)
 if(!$join){return $null}
 $last=$join.CommandElements[-1]
 if($last -isnot [Management.Automation.Language.StringConstantExpressionAst]){return $null}
 $name=$last.Value
 if($name.StartsWith('tools/')){return [IO.Path]::GetFullPath((Join-Path (Join-Path $Directory '../../..') $name))}
 [IO.Path]::GetFullPath((Join-Path $Directory $name))
}
function Closure-Assignment($Node){for($at=$Node.Parent;$at;$at=$at.Parent){if($at -is [Management.Automation.Language.AssignmentStatementAst]){return $at}}}
function Closure-Branch($Node){
 for($at=$Node;$at.Parent;$at=$at.Parent){if($at -is [Management.Automation.Language.StatementBlockAst] -and $at.Parent -is [Management.Automation.Language.IfStatementAst]){return @{owner=$at.Parent;block=$at}}}
}
function Closure-Unwrap($Node){
 while($Node){
  if($Node -is [Management.Automation.Language.CommandExpressionAst]){$Node=$Node.Expression}
  elseif($Node -is [Management.Automation.Language.ParenExpressionAst]){$Node=$Node.Pipeline}
  elseif($Node -is [Management.Automation.Language.PipelineAst] -and $Node.PipelineElements.Count -eq 1){$Node=$Node.PipelineElements[0]}
  elseif($Node -is [Management.Automation.Language.ArrayExpressionAst] -and $Node.SubExpression.Statements.Count -eq 1){$Node=$Node.SubExpression.Statements[0]}
  elseif($Node -is [Management.Automation.Language.ReturnStatementAst]){$Node=$Node.Pipeline}
  else{break}
 };$Node
}
function Closure-Binding($Node,$Name,$Bindings){
 if($Bindings.ContainsKey($Name)){$binding=$Bindings[$Name];if($binding.ContainsKey('unresolved')){throw ('Unsupported imported-function loop: '+$binding.unresolved)};return $binding}
 for($scope=Closure-Scope $Node;$scope;$scope=Closure-Scope $scope.Parent){
  $assignments=@($scope.FindAll({param($n)$n -is [Management.Automation.Language.AssignmentStatementAst] -and $n.Left -is [Management.Automation.Language.VariableExpressionAst] -and $n.Left.VariablePath.UserPath -eq $Name -and $n.Extent.EndOffset -le $Node.Extent.StartOffset -and (Closure-SameScope $n $scope)},$true)|Sort-Object {$_.Extent.StartOffset} -Descending)
  if($assignments.Count){return @{node=$assignments[0].Right;bindings=$Bindings}}
  $parameters=@($scope.ParamBlock.Parameters)+@($scope.Parent.Parameters)
  $parameter=@($parameters|Where-Object {$_.Name.VariablePath.UserPath -eq $Name})
  if($parameter.Count){
   if($parameter[0].DefaultValue){return @{node=$parameter[0].DefaultValue;bindings=$Bindings}}
   if(!$scope.Parent){
    $runner=Read-ClosureAst (Join-Path ([IO.Path]::GetDirectoryName($scope.Extent.File)) '../scripts/test-without-gpu.ps1')
    $values=@(@(foreach($call in $runner.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.Extent.Text.Contains([IO.Path]::GetFileName($scope.Extent.File))},$true)){
     for($i=0;$i -lt $call.CommandElements.Count-1;$i++){if($call.CommandElements[$i] -is [Management.Automation.Language.CommandParameterAst] -and $call.CommandElements[$i].ParameterName -eq $Name){Closure-Values $call.CommandElements[$i+1] $runner @{}}}
    })|Select-Object -Unique)
    if($values.Count -eq 1){return @{values=$values}}
   }
   return $null
  }
 }
}
function Closure-Call($Call,$Tree,$Bindings){
 $name=$Call.GetCommandName();$definitions=@($Tree.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true))
 if($definitions.Count -ne 1){return $null}
 $definition=$definitions[0];$parameters=@($definition.Parameters)+@($definition.Body.ParamBlock.Parameters);$bound=@{}+$Bindings
 for($i=0;$i -lt $parameters.Count;$i++){
  if($Call.CommandElements.Count -gt ($i+1)){$bound[$parameters[$i].Name.VariablePath.UserPath]=@{node=$Call.CommandElements[$i+1];bindings=$Bindings}}
 }
 @{node=$definition.Body.EndBlock.Statements[-1];bindings=$bound}
}
function Closure-Items($Node,$Tree,$Bindings){
 $Node=Closure-Unwrap $Node
 if($Node -is [Management.Automation.Language.ArrayLiteralAst]){return @($Node.Elements)}
 if($Node -is [Management.Automation.Language.ArrayExpressionAst]){return @($Node.SubExpression.Statements|ForEach-Object {Closure-Items $_ $Tree $Bindings})}
 if($Node -is [Management.Automation.Language.IfStatementAst]){
  foreach($clause in $Node.Clauses){
   $condition=Closure-Unwrap $clause.Item1
   if($condition -isnot [Management.Automation.Language.BinaryExpressionAst] -or $condition.Operator -notin @('Ieq','Ceq')){throw 'Unsupported imported-function conditional list'}
   $left=@(Closure-Values $condition.Left $Tree $Bindings);$right=@(Closure-Values $condition.Right $Tree $Bindings)
   if($left.Count -ne 1 -or $right.Count -ne 1){throw 'Ambiguous imported-function conditional list'}
   if($left[0] -eq $right[0]){return @($clause.Item2.Statements|ForEach-Object {Closure-Items $_ $Tree $Bindings})}
  }
  if($Node.ElseClause){return @($Node.ElseClause.Statements|ForEach-Object {Closure-Items $_ $Tree $Bindings})};return @()
 }
 if($Node -is [Management.Automation.Language.SubExpressionAst]){return @($Node.SubExpression.Statements|ForEach-Object {Closure-Items $_ $Tree $Bindings})}
 if($Node -is [Management.Automation.Language.VariableExpressionAst]){
  $binding=Closure-Binding $Node $Node.VariablePath.UserPath $Bindings
  if($binding){return @(Closure-Items $binding.node $Tree $binding.bindings)}
 }
 ,$Node
}
function Closure-Values($Node,$Tree,$Bindings){
 $Node=Closure-Unwrap $Node
 if(!$Node){return @()}
 if($Node -is [Management.Automation.Language.StringConstantExpressionAst]){return @($Node.Value)}
 if($Node -is [Management.Automation.Language.VariableExpressionAst]){
  if($Node.VariablePath.UserPath -eq 'PSScriptRoot'){return @([IO.Path]::GetDirectoryName($Tree.Extent.File))}
  $binding=Closure-Binding $Node $Node.VariablePath.UserPath $Bindings
  if(!$binding){throw "Unresolved imported-function value: $($Node.Extent.Text)"};if($binding.ContainsKey('values')){return @($binding.values)};return @(Closure-Values $binding.node $Tree $binding.bindings)
 }
 if($Node -is [Management.Automation.Language.BinaryExpressionAst] -and $Node.Operator -eq 'Plus'){
  $left=@(Closure-Values $Node.Left $Tree $Bindings);$right=@(Closure-Values $Node.Right $Tree $Bindings)
  foreach($a in $left){foreach($b in $right){$a+$b}};return
 }
 if($Node -is [Management.Automation.Language.MemberExpressionAst]){
  if($Node.Member.Value -eq 'Path'){return @(Closure-Values $Node.Expression $Tree $Bindings)}
  $target=Closure-Unwrap $Node.Expression
  if($target -is [Management.Automation.Language.VariableExpressionAst]){$binding=Closure-Binding $target $target.VariablePath.UserPath $Bindings;if($binding){$target=Closure-Unwrap $binding.node}}
  if($target -is [Management.Automation.Language.HashtableAst]){
   foreach($pair in $target.KeyValuePairs){if($pair.Item1.Value -eq $Node.Member.Value){return @(Closure-Values $pair.Item2.Statements[0] $Tree $Bindings)}}
  }
 }
 if($Node -is [Management.Automation.Language.CommandAst]){
  if($Node.GetCommandName() -eq 'Join-Path'){
   $parent=@(Closure-Values $Node.CommandElements[1] $Tree $Bindings);$child=@(Closure-Values $Node.CommandElements[2] $Tree $Bindings)
   foreach($a in $parent){foreach($b in $child){[IO.Path]::GetFullPath((Join-Path $a $b))}};return
  }
  if($Node.GetCommandName() -eq 'Resolve-Path'){return @(Closure-Values $Node.CommandElements[1] $Tree $Bindings)}
  if($Node.GetCommandName() -eq 'Split-Path' -and $Node.CommandElements.Count -eq 3 -and $Node.CommandElements[1].ParameterName -eq 'Parent'){return @(Closure-Values $Node.CommandElements[2] $Tree $Bindings|ForEach-Object {[IO.Path]::GetDirectoryName($_)})}
 }
 throw "Unsupported imported-function value: $($Node.Extent.Text)"
}
function Closure-Contexts($Site,$Tree,$Bindings){
 $loops=@();for($at=$Site.Parent;$at;$at=$at.Parent){if($at -is [Management.Automation.Language.ForEachStatementAst]){$loops=@($at)+$loops}}
 $contexts=@($Bindings)
 foreach($loop in $loops){
  $next=@();foreach($context in $contexts){
   try{$items=@(Closure-Items $loop.Condition $Tree $context)}catch{$bound=@{}+$context;$bound[$loop.Variable.VariablePath.UserPath]=@{unresolved=$loop.Condition.Extent.Text};$next+=,$bound;continue}
   foreach($item in $items){$bound=@{}+$context;$bound[$loop.Variable.VariablePath.UserPath]=@{node=$item;bindings=$context};$next+=,$bound}
  }
  $contexts=$next
 }
 $contexts
}
function Closure-Provider($Node,$Tree,$Bindings){
 $Node=Closure-Unwrap $Node
 if($Node -is [Management.Automation.Language.VariableExpressionAst]){
  $binding=Closure-Binding $Node $Node.VariablePath.UserPath $Bindings
  if($binding){return @(Closure-Provider $binding.node $Tree $binding.bindings)}
 }
 if($Node -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $Node.Member.Value -in @('ParseFile','ParseInput')){
  $input=Closure-Unwrap $Node.Arguments[0]
  if($input -is [Management.Automation.Language.VariableExpressionAst] -and $input.VariablePath.UserPath.StartsWith('env:')){
   $variable=$input.VariablePath.UserPath;$path=[Environment]::GetEnvironmentVariable($variable.Substring(4),'Process')
   if(!$path){$script:closureExternal[$Node.Extent.File+':'+$Node.Extent.StartLineNumber]="External AST comparison provider not supplied: $([IO.Path]::GetFileName($Node.Extent.File)):$($Node.Extent.StartLineNumber) `$$variable (outside repository fixture ownership)";return @()}
   if(!(Test-Path -LiteralPath $path -PathType Leaf)){throw "External AST comparison provider is unreadable: `$$variable"}
   return @([IO.Path]::GetFullPath($path))
  }
  if($Node.Member.Value -eq 'ParseInput'){
   if($input -is [Management.Automation.Language.VariableExpressionAst]){$binding=Closure-Binding $input $input.VariablePath.UserPath $Bindings;if($binding){$input=Closure-Unwrap $binding.node}}
   if($input -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $input.Member.Value -eq 'ReadAllText'){$input=$input.Arguments[0]}
   elseif($input -is [Management.Automation.Language.CommandAst] -and $input.GetCommandName() -eq 'Get-Content'){$input=$input.CommandElements[1]}
   else{throw 'Unsupported imported-function ParseInput source'}
  }
  return @(Closure-Values $input $Tree $Bindings)
 }
 if($Node -is [Management.Automation.Language.CommandAst]){
  $call=Closure-Call $Node $Tree $Bindings
  if($call){return @(Closure-Provider $call.node $Tree $call.bindings)}
 }
 throw "Unsupported imported-function AST provider: $($Node.Extent.Text)"
}
function Closure-Selection($Node,$Tree,$Bindings){
 $Node=Closure-Unwrap $Node
 if($Node -is [Management.Automation.Language.VariableExpressionAst]){
  $binding=Closure-Binding $Node $Node.VariablePath.UserPath $Bindings
  if($binding){return @(Closure-Selection $binding.node $Tree $binding.bindings)}
  return @()
 }
 if($Node -is [Management.Automation.Language.IndexExpressionAst]){return @(Closure-Selection $Node.Target $Tree $Bindings)}
 if($Node -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $Node.Member.Value -in @('Find','FindAll') -and $Node.Arguments[0].Extent.Text.Contains('FunctionDefinitionAst')){return @(@{selector=$Node;bindings=$Bindings})}
 @()
}
function Closure-TextSelections($Node,$Tree,$Bindings){
 $Node=Closure-Unwrap $Node
 if(!$Node){return @()}
 if($Node -is [Management.Automation.Language.VariableExpressionAst]){
  $binding=Closure-Binding $Node $Node.VariablePath.UserPath $Bindings
  if($binding){return @(Closure-TextSelections $binding.node $Tree $binding.bindings)};return @()
 }
 if($Node -is [Management.Automation.Language.MemberExpressionAst] -and $Node.Member.Value -eq 'Text' -and $Node.Expression -is [Management.Automation.Language.MemberExpressionAst] -and $Node.Expression.Member.Value -eq 'Extent'){
  return @(Closure-Selection $Node.Expression.Expression $Tree $Bindings)
 }
 if($Node -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $Node.Member.Value -eq 'Replace'){return @(Closure-TextSelections $Node.Expression $Tree $Bindings)}
 if($Node -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $Node.Member.Value -eq 'Substring'){if($Node.Arguments[0].Extent.Text.Contains('function ')){return @(Closure-TextSelections $Node.Expression $Tree $Bindings)};return @()}
 if($Node -is [Management.Automation.Language.BinaryExpressionAst]){return @((Closure-TextSelections $Node.Left $Tree $Bindings);(Closure-TextSelections $Node.Right $Tree $Bindings))}
 if($Node -is [Management.Automation.Language.InvokeMemberExpressionAst]){
  $whole=@(Closure-TextSelections $Node.Expression $Tree $Bindings);if(!$whole.Count){$whole=@(Closure-Selection $Node.Expression $Tree $Bindings)}
  if($whole.Count){throw "Unsupported FUNCTION text transform: $($Node.Extent.Text)"}
 }
 if($Node -is [Management.Automation.Language.CommandAst]){
  $call=Closure-Call $Node $Tree $Bindings
  if($call){return @(Closure-TextSelections $call.node $Tree $call.bindings)}
 }
 @()
}
function Get-ClosureImports($Tree){
 $imports=[Collections.Generic.List[object]]::new()
 $evaluators=@($Tree.FindAll({param($n)($n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Invoke-Expression') -or ($n -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $n.Member.Value -eq 'Create' -and $n.Expression.Extent.Text -match '(?i)scriptblock')},$true))
 foreach($evaluation in $evaluators){
  $expression=if($evaluation -is [Management.Automation.Language.CommandAst]){$evaluation.CommandElements[1]}else{$evaluation.Arguments[0]}
  $scope=Closure-Scope $evaluation;$calls=@(@{site=$evaluation;bindings=@{}})
  if($scope.Parent -is [Management.Automation.Language.FunctionDefinitionAst]){
   $loader=$scope.Parent.Name
   $calls=@($Tree.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq $loader},$true)|ForEach-Object {$call=Closure-Call $_ $Tree @{};@{site=$_;bindings=$call.bindings}})
  }
  foreach($call in $calls){foreach($callerBindings in @(Closure-Contexts $call.site $Tree $call.bindings)){foreach($bindings in $(if([object]::ReferenceEquals($evaluation,$call.site)){@($callerBindings)}else{@(Closure-Contexts $evaluation $Tree $callerBindings)})){
   foreach($selection in @(Closure-TextSelections $expression $Tree $bindings)){
    $selector=$selection.selector;$bindings=$selection.bindings
    $providers=@(Closure-Provider $selector.Expression $Tree $bindings)
    $test=Closure-Unwrap $selector.Arguments[0]
    $named=$test.Find({param($n)$n -is [Management.Automation.Language.BinaryExpressionAst] -and $n.Operator -in @('Ieq','Ceq') -and $n.Left -is [Management.Automation.Language.MemberExpressionAst] -and $n.Left.Member.Value -eq 'Name'},$true)
    if(!$named){$conditions=@($test.FindAll({param($n)$n -is [Management.Automation.Language.BinaryExpressionAst]},$true));if($conditions.Count -ne 1 -or $conditions[0].Operator -ne 'Is'){throw "Unsupported FUNCTION import selector: $($test.Extent.Text)"}}
    foreach($provider in $providers){
     $ast=Read-ClosureAst $provider
     $names=if($named){@(Closure-Values $named.Right $Tree $bindings)}else{@($ast.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst]},$true)|ForEach-Object Name)}
     if(!$names.Count){throw 'Empty imported function selection'}
     $binding=Closure-Assignment $selector
     $branch=Closure-Branch $evaluation
     $optional=$branch -and $binding -and @($branch.owner.Clauses|Where-Object {$_.Item1.Extent.Text -eq $binding.Left.Extent.Text -and [object]::ReferenceEquals($_.Item2,$branch.block)}).Count
     if($optional){$names=@($names|Where-Object {$name=$_;@($ast.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true)).Count})}
     if(!$names.Count){continue}
     $branch=Closure-Branch $call.site
     if($branch -and $binding){
      $condition=@($branch.owner.Clauses|Where-Object {[object]::ReferenceEquals($_.Item2,$branch.block)}|ForEach-Object Item1)
      if($optional -or ($condition.Count -eq 1 -and $condition[0].Extent.Text -match ('^'+[regex]::Escape($binding.Left.Extent.Text)+'\.Count\s*-eq\s*1$'))){$branch=$null}
     }
     $imports.Add(@{source=$provider;names=$names;scope=(Closure-Scope $call.site);branch=$branch;site=$call.site;bindings=$bindings})
    }
   }
  }}}
 }
 $imports.ToArray()
}
function Add-ClosureDefinitions($Scope,$Directory,$Available,$Mocks,$Seen,$Bindings=@{}){
 $tree=$Scope;while($tree.Parent){$tree=$tree.Parent}
 foreach($definition in $Scope.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst]},$true)){
  if(Closure-SameScope $definition $Scope){$Available[$definition.Name -replace '^(script|global):','']=$definition}
 }
 foreach($write in $Scope.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Set-Content'},$true)){
  if(!(Closure-SameScope $write $Scope) -or $write.CommandElements.Count -ne 3){continue}
  $path=Closure-Path $write.CommandElements[1] $Directory
  if($path -and $path.EndsWith('.ps1') -and $write.CommandElements[2] -is [Management.Automation.Language.StringConstantExpressionAst]){
   $tokens=$null;$errors=$null;$mock=[Management.Automation.Language.Parser]::ParseInput($write.CommandElements[2].Value,[ref]$tokens,[ref]$errors)
   if($errors.Count){throw 'Invalid declared AST-import mock source'};$Mocks[[IO.Path]::GetFileName($path)]=$mock
  }
 }
 foreach($write in $Scope.FindAll({param($n)$n -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $n.Member.Value -eq 'WriteAllText'},$true)){
  if(!(Closure-SameScope $write $Scope) -or $write.Arguments.Count -ne 2 -or $write.Arguments[1] -isnot [Management.Automation.Language.StringConstantExpressionAst]){continue}
  $path=Closure-Path $write.Arguments[0] $Directory
  if($path -and $path.EndsWith('.ps1')){
   $tokens=$null;$errors=$null;$mock=[Management.Automation.Language.Parser]::ParseInput($write.Arguments[1].Value,[ref]$tokens,[ref]$errors)
   if($errors.Count){throw 'Invalid declared AST-import mock source'};$Mocks[[IO.Path]::GetFileName($path)]=$mock
  }
 }
 foreach($command in $Scope.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.InvocationOperator -eq 'Dot'},$true)){
  if(!(Closure-SameScope $command $Scope)){continue}
  if($command.Extent.Text -match '(?i)scriptblock\]::Create'){continue}
  $expression=Closure-Unwrap $command.CommandElements[0]
  if($expression -is [Management.Automation.Language.ScriptBlockExpressionAst]){continue}
  if($expression -is [Management.Automation.Language.VariableExpressionAst]){
   $items=@(foreach($context in @(Closure-Contexts $command $tree $Bindings)){Closure-Items $expression $tree $context})
   if($items.Count -and !@($items|Where-Object {$_ -isnot [Management.Automation.Language.ScriptBlockExpressionAst]}).Count){continue}
  }
  $path=Closure-Path $expression $Directory
  if($path -and !$Mocks.ContainsKey([IO.Path]::GetFileName($path))){$resolved=@(Closure-Values $expression $tree $Bindings);if($resolved.Count -ne 1){throw 'Ambiguous dot-source closure'};$path=$resolved[0]}
  if(!$path -and [IO.Path]::GetFileName([IO.Path]::GetDirectoryName($tree.Extent.File)) -eq 'tests'){continue}
  if(!$path){throw "Unsupported dot-source closure: $($command.Extent.Text)"}
  if($Seen.ContainsKey($path)){continue};$Seen[$path]=$true
  $dependency=if($Mocks.ContainsKey([IO.Path]::GetFileName($path))){$Mocks[[IO.Path]::GetFileName($path)]}else{Read-ClosureAst $path}
  Add-ClosureDefinitions $dependency ([IO.Path]::GetDirectoryName($path)) $Available $Mocks $Seen
 }
}
function Test-ClosureImport($Import,$Peers,$Directory,$Consumer){
 $available=@{};$mocks=@{};$seen=@{};$declared=@{};$missing=[Collections.Generic.List[string]]::new()
 $test=[IO.Path]::GetFileName([IO.Path]::GetDirectoryName($Import.site.Extent.File)) -eq 'tests'
 if($Consumer){Add-ClosureDefinitions $Consumer ([IO.Path]::GetDirectoryName($Consumer.Extent.File)) $available $mocks $seen}
 $scopes=@();for($scope=$Import.scope;$scope;$scope=Closure-Scope $scope.Parent){$scopes=@($scope)+$scopes}
 foreach($scope in $scopes){Add-ClosureDefinitions $scope $Directory $available $mocks $seen $Import.bindings}
 foreach($peer in $Peers){
  if(!@($scopes|Where-Object {[object]::ReferenceEquals($_,$peer.scope)}).Count){continue}
  if($peer.branch -and !$Import.branch){continue}
  if($peer.branch -and $Import.branch -and [object]::ReferenceEquals($peer.branch.owner,$Import.branch.owner) -and ![object]::ReferenceEquals($peer.branch.block,$Import.branch.block)){continue}
  $provider=Read-ClosureAst $peer.source
  if($test){
   if(!$script:closureProviders.ContainsKey($peer.source)){
    $definitions=@{};$providerMocks=@{};$providerSeen=@{}
    Add-ClosureDefinitions $provider ([IO.Path]::GetDirectoryName($peer.source)) $definitions $providerMocks $providerSeen
    $script:closureProviders[$peer.source]=$definitions
   }
   foreach($name in $script:closureProviders[$peer.source].Keys){$declared[$name]=$true}
  }
  foreach($name in $peer.names){
   $definitions=@($provider.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true))
   if($definitions.Count -ne 1){throw "Expected one imported $name in $($peer.source)"};$available[$name]=$definitions[0]
  }
 }
 foreach($imported in $Import.names){
  $provider=Read-ClosureAst $Import.source
  $item=@{name=$imported;node=$provider.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $imported},$true)};$body=$item.node.Body
  Add-ClosureDefinitions $body ([IO.Path]::GetDirectoryName($item.node.Extent.File)) $available $mocks $seen $Import.bindings
  foreach($command in $body.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst]},$true)){
   $name=$command.GetCommandName()
   if(!$name){
    if($test){continue}
    if($command.InvocationOperator -eq 'Dot'){continue}
    $path=Closure-Path $command.CommandElements[0] ([IO.Path]::GetDirectoryName($item.node.Extent.File))
    if($path -and (Test-Path -LiteralPath $path)){continue}
    $variable=$command.CommandElements[0]
    $parameters=@($item.node.Parameters)+@($body.ParamBlock.Parameters)
    if($variable -is [Management.Automation.Language.VariableExpressionAst] -and @($parameters|Where-Object {$_.Name.VariablePath.UserPath -eq $variable.VariablePath.UserPath -and $_.StaticType -eq [scriptblock]}).Count){continue}
    throw "Unresolved dynamic command in imported $($item.name): $($command.Extent.Text)"
   }
   if($available.ContainsKey($name)){continue}
   if($test -and !$declared.ContainsKey($name)){continue}
   $builtin=Get-Command $name -CommandType Cmdlet,Alias -ErrorAction SilentlyContinue
   if($builtin){continue}
   $missing.Add("Missing imported function dependency: $([IO.Path]::GetFileName($Import.site.Extent.File)):$($Import.site.Extent.StartLineNumber) $($item.name) -> $name")
  }
 }
 if($missing.Count){throw ($missing -join "`n")}
}
function Test-FixtureImportClosures($Directory){
 $tests=[IO.Path]::GetFullPath((Join-Path $Directory '../tests'))
 $ownedControls=Join-Path $tests 'fixture-import-closure.ps1'
 $all=@(Get-ChildItem -LiteralPath $Directory,$tests -Filter '*.ps1' -File|Where-Object {$_.FullName -ne $ownedControls});$sites=0;$failures=[Collections.Generic.List[string]]::new()
 $consumersByPath=@{}
 foreach($file in $all){
  $candidate=Read-ClosureAst $file.FullName
  foreach($dot in $candidate.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.InvocationOperator -eq 'Dot'},$true)){
   $path=Closure-Path $dot.CommandElements[0] $file.DirectoryName
   if($path){if(!$consumersByPath.ContainsKey($path)){$consumersByPath[$path]=@()};$consumersByPath[$path]+=,$candidate}
  }
 }
 foreach($file in $all){
  try{
  $fileDirectory=$file.DirectoryName;$tree=Read-ClosureAst $file.FullName;$imports=@(Get-ClosureImports $tree)
  foreach($import in $imports){
   $consumers=@($consumersByPath[$file.FullName]|Where-Object {$_})
   if(!$consumers.Count){Test-ClosureImport $import $imports $fileDirectory $null}else{foreach($consumer in $consumers){Test-ClosureImport $import $imports $fileDirectory $consumer}}
   $sites++
  }
  }catch{$failures.Add("$($file.Name): $($_.Exception.Message)")}
 }
 foreach($external in $script:closureExternal.Values){Write-Output $external}
 if($failures.Count){throw ($failures -join "`n")}
 "AST-imported function closure passed $sites actual import sites"
}
function Test-ClosureControls{
 $directory=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'closure-controls'))
 $provider=Join-Path $directory 'provider.ps1';$consumer=Join-Path $directory 'consumer.ps1'
 $tokens=$null;$errors=$null
 $script:closureAsts[$provider]=[Management.Automation.Language.Parser]::ParseInput('function Entry {Close-Preferences};function Close-Preferences {}',$provider,[ref]$tokens,[ref]$errors)
 $prefix='$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot ''provider.ps1''),[ref]$null,[ref]$null);'
 $selector='foreach($name in @({0})){{$fn=$ast.Find({{param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name}},$true);Invoke-Expression $fn.Extent.Text}}'
 foreach($case in @(
  @{name='complete imported closure';names="'Entry','Close-Preferences'";local='';missing=$false},
  @{name='missing Close-Preferences';names="'Entry'";local='';missing=$true},
  @{name='consumer-local definition';names="'Entry'";local='function Close-Preferences {}';missing=$false},
  @{name='independent child scope cannot mask';names="'Entry'";local='& {function Close-Preferences {}}';missing=$true}
 )){
  $tree=[Management.Automation.Language.Parser]::ParseInput(($prefix+($selector -f $case.names)+$case.local),$consumer,[ref]$tokens,[ref]$errors)
  if($errors.Count){throw ($errors|Out-String)}
  $imports=@(Get-ClosureImports $tree);$failure=$null
  try{foreach($import in $imports){Test-ClosureImport $import $imports $directory $null}}catch{$failure=$_.Exception.Message}
  if($case.missing){if(!$failure -or !$failure.Contains('Entry -> Close-Preferences')){throw "Closure control did not detect $($case.name): $failure"}}
  elseif($failure){throw "Closure control rejected $($case.name): $failure"}
 }
 foreach($case in @(
  @{name='default provider';source=('param([string]$Fixture=(Join-Path $PSScriptRoot ''provider.ps1''));$ast=[Management.Automation.Language.Parser]::ParseFile($Fixture,[ref]$null,[ref]$null);'+($selector -f "'Entry','Close-Preferences'"));expected=$null},
  @{name='all functions';source=($prefix+'foreach($fn in $ast.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst]},$true)){Invoke-Expression $fn.Extent.Text}');expected=$null},
  @{name='unresolved owned provider';source=('$ast=[Management.Automation.Language.Parser]::ParseFile($unknown,[ref]$null,[ref]$null);'+($selector -f "'Entry'"));expected='Unresolved imported-function value'},
  @{name='unresolved names';source=($prefix+'$fn=$ast.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $unknown},$true);Invoke-Expression $fn.Extent.Text');expected='Unresolved imported-function value'},
  @{name='unsupported selector';source=($prefix+'$fn=$ast.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -like "Entry*"},$true);Invoke-Expression $fn.Extent.Text');expected='Unsupported FUNCTION import selector'},
  @{name='unsupported whole text transform';source=($prefix+'$fn=$ast.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq "Entry"},$true);Invoke-Expression $fn.Extent.Text.ToUpperInvariant()');expected='Unsupported FUNCTION text transform'},
  @{name='unsupported owned loop shadow';source=($prefix+'foreach($name in "Close-Preferences"){foreach($name in $(if($unknown -ne $null){"Entry"})){$fn=$ast.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true);Invoke-Expression $fn.Extent.Text}}');expected='Unsupported imported-function loop'}
 )){
  $tree=[Management.Automation.Language.Parser]::ParseInput($case.source,$consumer,[ref]$tokens,[ref]$errors)
  if($errors.Count){throw ($errors|Out-String)};$failure=$null
  try{$imports=@(Get-ClosureImports $tree);foreach($import in $imports){Test-ClosureImport $import $imports $directory $null}}catch{$failure=$_.Exception.Message}
  if($case.expected){if(!$failure -or !$failure.Contains($case.expected)){throw "Closure control did not detect $($case.name): $failure"}}
  elseif($failure){throw "Closure control rejected $($case.name): $failure"}
 }
 $provider=Join-Path $directory 'scripts/provider.ps1';$dependency=Join-Path $directory 'scripts/dependency.ps1';$consumer=Join-Path $directory 'tests/consumer.ps1'
 $script:closureAsts[$provider]=[Management.Automation.Language.Parser]::ParseInput('. (Join-Path $PSScriptRoot ''dependency.ps1'');function Entry {Close-Preferences}',$provider,[ref]$tokens,[ref]$errors)
 $script:closureAsts[$dependency]=[Management.Automation.Language.Parser]::ParseInput('function Close-Preferences {}',$dependency,[ref]$tokens,[ref]$errors)
 foreach($local in @('','function Close-Preferences {}')){
  $source='$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot ''../scripts/provider.ps1''),[ref]$null,[ref]$null);'+($selector -f "'Entry'")+$local
  $tree=[Management.Automation.Language.Parser]::ParseInput($source,$consumer,[ref]$tokens,[ref]$errors);$failure=$null
  try{$imports=@(Get-ClosureImports $tree);foreach($import in $imports){Test-ClosureImport $import $imports ([IO.Path]::GetDirectoryName($consumer)) $null}}catch{$failure=$_.Exception.Message}
  if(!$local){if(!$failure -or !$failure.Contains('Entry -> Close-Preferences')){throw 'Provider dot-source definitions incorrectly became consumer imports'}}
  elseif($failure){throw "Provider dependency ignored the consumer stub: $failure"}
 }
 'AST-imported function closure controls passed 13 cases'
}
function Test-ActualImportControls($Directory){
 foreach($case in @(
  @{file='exercise-touch-gestures.ps1';before="'Close-Preferences',";after='';expected='Set-PinchPreferences -> Close-Preferences'},
  @{file='exercise-tool-features.ps1';before="@('Box','Tile'";after="@('Control','Box','Tile'";expected='Expected one imported Control'},
  @{file='../tests/paper-relief.ps1';before="'Texture-Begin','Texture-Observed',";after='';expected='Texture-Open -> Texture-Begin'}
 )){
  $path=Join-Path $Directory $case.file;$source=[IO.File]::ReadAllText($path)
  $tokens=$null;$errors=$null;$tree=[Management.Automation.Language.Parser]::ParseInput($source.Replace($case.before,$case.after),$path,[ref]$tokens,[ref]$errors)
  if($errors.Count){throw ($errors|Out-String)}
  $imports=@(Get-ClosureImports $tree);$failure=$null
  try{foreach($import in $imports){Test-ClosureImport $import $imports $Directory $null}}catch{$failure=$_.Exception.Message}
  if(!$failure -or !$failure.Contains($case.expected)){throw "Actual import negative did not detect $($case.file): $failure"}
 }
 'Actual imported-function negatives passed Touch, feature and paper helpers'
}
Test-ClosureControls
if(!$SelfTestOnly){Test-ActualImportControls ([IO.Path]::GetFullPath($Scripts))}
if(!$SelfTestOnly){Test-FixtureImportClosures ([IO.Path]::GetFullPath($Scripts))}
