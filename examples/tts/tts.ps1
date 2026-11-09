param([string]$Out)
# Génère des extraits parlés (texte original) avec l'heure exacte de chaque mot.
Add-Type -AssemblyName System.Speech
$tests = @(
  @{ name = 'fr'; voice = 'Microsoft Hortense Desktop'; text = "Le petit lapin blanc traverse la forêt au lever du jour. Il porte un chapeau noir et chante doucement. Les oiseaux répondent, puis le silence revient. Un renard curieux le suit de loin, sans faire de bruit. Au bord de la rivière, le lapin s'arrête, regarde l'eau claire et reprend sa chanson. Ce matin, tout le monde écoute." },
  @{ name = 'en'; voice = 'Microsoft Zira Desktop'; text = "A small white rabbit walks through the forest at dawn. He wears a black hat and sings softly. The birds answer, then everything is quiet again. A curious fox follows him from far away without a sound. By the river, the rabbit stops, looks at the clear water and starts his song again. This morning, everybody listens." }
)
foreach ($t in $tests) {
  $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
  $synth.SelectVoice($t.voice)
  $synth.Rate = -1
  $fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo 16000, ([System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen), ([System.Speech.AudioFormat.AudioChannel]::Mono)
  $wav = Join-Path $Out "$($t.name).wav"
  $synth.SetOutputToWaveFile($wav, $fmt)
  $words = New-Object System.Collections.ArrayList
  Register-ObjectEvent -InputObject $synth -EventName SpeakProgress -SourceIdentifier "sp_$($t.name)" | Out-Null
  $synth.Speak($t.text)
  $synth.SetOutputToNull()
  foreach ($e in (Get-Event -SourceIdentifier "sp_$($t.name)")) {
    $a = $e.SourceEventArgs
    [void]$words.Add(@{ text = $a.Text; start = $a.AudioPosition.TotalSeconds })
    Remove-Event -EventIdentifier $e.EventIdentifier
  }
  Unregister-Event -SourceIdentifier "sp_$($t.name)"
  $synth.Dispose()
  $json = @{ text = $t.text; words = $words } | ConvertTo-Json -Depth 4
  [IO.File]::WriteAllText((Join-Path $Out "$($t.name).json"), $json, (New-Object Text.UTF8Encoding $false))
  "{0} : {1} mots, {2:N1} s" -f $t.name, $words.Count, ((Get-Item $wav).Length - 44) / 32000
}
