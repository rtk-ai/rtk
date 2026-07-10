param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $ProbeArgs
)

for ($i = 0; $i -lt $ProbeArgs.Count; $i++) {
    Write-Output ("arg{0}={1}" -f $i, $ProbeArgs[$i])
}
