param([string]$Out)
# Extraits parlés avec des phrases « parasites » absentes du texte fourni à l'aligneur
# (comme les chœurs, rires et répétitions absents des paroles LRCLIB).
# Lignes commençant par « - » : prononcées mais exclues du texte et de la vérité.
Add-Type -AssemblyName System.Speech
$tests = @(
  @{ name = 'fr_extra'; voice = 'Microsoft Hortense Desktop'; lines = @(
    '- Ha ha ha ha ha ha.',
    'Le petit lapin blanc traverse la forêt au lever du jour.',
    'Il porte un chapeau noir et chante doucement.',
    '- Oh oh oh, chante doucement, chante doucement.',
    'Les oiseaux répondent, puis le silence revient.',
    '- Le silence revient, le silence revient, oui.',
    'Un renard curieux le suit de loin, sans faire de bruit.',
    'Au bord de la rivière, le lapin s''arrête.',
    '- Ha ha ha, hé hé hé, ha ha.',
    'Il regarde l''eau claire et reprend sa chanson.',
    'Ce matin, tout le monde écoute.',
    '- Tout le monde écoute, tout le monde écoute.') },
  @{ name = 'en_extra'; voice = 'Microsoft Zira Desktop'; lines = @(
    '- Ha ha ha ha ha ha.',
    'A small white rabbit walks through the forest at dawn.',
    'He wears a black hat and sings softly.',
    '- Oh oh oh, sings softly, sings softly.',
    'The birds answer, then everything is quiet again.',
    '- Quiet again, quiet again, yeah.',
    'A curious fox follows him from far away without a sound.',
    'By the river, the rabbit stops.',
    '- Ha ha ha, hey hey hey, ha ha.',
    'He looks at the clear water and starts his song again.',
    'This morning, everybody listens.',
    '- Everybody listens, everybody listens.') }
)
foreach ($t in $tests) {
  # Texte prononcé et plages de caractères des lignes conservées.
  $spoken = ''; $keep = @(); $kept = @()
  foreach ($l in $t.lines) {
    $extra = $l.StartsWith('- ')
    $s = if ($extra) { $l.Substring(2) } else { $l }
    $from = $spoken.Length
    $spoken += $s + ' '
    if (-not $extra) { $keep += , @($from, $spoken.Length); $kept += $s }
  }
  $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
  $synth.SelectVoice($t.voice)
  $synth.Rate = -1
  $fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo 16000, ([System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen), ([System.Speech.AudioFormat.AudioChannel]::Mono)
  $wav = Join-Path $Out "$($t.name).wav"
  $synth.SetOutputToWaveFile($wav, $fmt)
  Register-ObjectEvent -InputObject $synth -EventName SpeakProgress -SourceIdentifier "sp_$($t.name)" | Out-Null
  $synth.Speak($spoken)
  $synth.SetOutputToNull()
  $words = New-Object System.Collections.ArrayList
  foreach ($e in (Get-Event -SourceIdentifier "sp_$($t.name)")) {
    $a = $e.SourceEventArgs
    $inKept = $false
    foreach ($r in $keep) { if ($a.CharacterPosition -ge $r[0] -and $a.CharacterPosition -lt $r[1]) { $inKept = $true } }
    if ($inKept) { [void]$words.Add(@{ text = $a.Text; start = $a.AudioPosition.TotalSeconds }) }
    Remove-Event -EventIdentifier $e.EventIdentifier
  }
  Unregister-Event -SourceIdentifier "sp_$($t.name)"
  $synth.Dispose()
  $json = @{ text = ($kept -join "`n"); words = $words } | ConvertTo-Json -Depth 4
  [IO.File]::WriteAllText((Join-Path $Out "$($t.name).json"), $json, (New-Object Text.UTF8Encoding $false))
  "$($t.name) : $($words.Count) mots conservés"
}
