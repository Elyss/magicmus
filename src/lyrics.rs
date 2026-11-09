//! Paroles synchronisées mot à mot.
//!
//! La voix isolée (mélange − accompagnement) est transcrite par Whisper (whisper.cpp)
//! en mode « un mot par segment », ce qui donne l'instant de chaque mot. Les mots sont
//! ensuite regroupés en lignes pour l'affichage karaoké.

use crate::setup::Whisper;
use crate::tools;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    /// Début et fin, en secondes depuis le début du morceau.
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Line {
    pub words: Vec<Word>,
}

impl Line {
    pub fn start(&self) -> f64 {
        self.words.first().map_or(0.0, |w| w.start)
    }
    pub fn end(&self) -> f64 {
        self.words.last().map_or(0.0, |w| w.end)
    }
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Lyrics {
    pub language: String,
    pub lines: Vec<Line>,
    /// Avance d'affichage réglée par l'utilisateur (s) : positif = paroles plus tôt.
    #[serde(default)]
    pub offset: f64,
    /// Origine du texte (« LRCLIB », « texte saisi », « reconnaissance vocale »).
    #[serde(default)]
    pub source: String,
}

impl Lyrics {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        // Tolère une marque BOM (fichier modifié dans un éditeur Windows).
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        Ok(serde_json::from_slice(bytes)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Copie avec l'avance d'affichage appliquée aux horodatages.
    pub fn shifted(&self) -> Lyrics {
        let mut l = self.clone();
        for line in &mut l.lines {
            for w in &mut line.words {
                w.start -= self.offset;
                w.end -= self.offset;
            }
        }
        l.offset = 0.0;
        l
    }

    /// Texte brut, un vers par ligne (pour l'éditeur de paroles).
    pub fn to_text(&self) -> String {
        self.lines.iter().map(Line::text).collect::<Vec<_>>().join("\n")
    }

    pub fn word_count(&self) -> usize {
        self.lines.iter().map(|l| l.words.len()).sum()
    }

    /// Indice de la ligne à afficher à l'instant `t` : la ligne en cours, sinon la
    /// prochaine si elle commence bientôt.
    pub fn line_at(&self, t: f64) -> Option<usize> {
        let next = self.lines.iter().position(|l| l.end() + 0.4 >= t)?;
        (self.lines[next].start() - t < 3.0).then_some(next)
    }
}

/// Nom du résultat brut de Whisper, conservé pour réaligner des paroles corrigées.
pub const WHISPER_JSON: &str = "whisper.json";

/// Transcrit la voix (`vocals` : stéréo entrelacé à `sample_rate`) et renvoie
/// (langue détectée, mots horodatés). Le résultat brut est gardé dans `work_dir`.
/// `language` : code ISO (« fr », « en »…) ou « auto ».
pub fn transcribe(
    whisper: &Whisper,
    vocals: &[f32],
    sample_rate: u32,
    work_dir: &Path,
    language: &str,
    progress: &mut dyn FnMut(f32),
) -> Result<(String, Vec<Word>)> {
    let wav = work_dir.join("voix_16k.wav");
    crate::media::write_wav_16k_mono(vocals, sample_rate, &wav)?;

    let out_base = work_dir.join("whisper");
    let json = out_base.with_extension("json");
    let _ = std::fs::remove_file(&json);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);

    let mut cmd = tools::command(&whisper.cli.to_string_lossy());
    cmd.arg("-m").arg(&whisper.model)
        .arg("-f").arg(&wav)
        .args(["-l", language, "-t", &threads.to_string()])
        .args(["-ml", "1", "-sow"]) // un mot par segment => horodatage de chaque mot
        .args(["-bs", "5", "-bo", "5"]) // recherche en faisceau : transcription plus fiable
        .args(["-oj", "-pp"])
        .arg("-of").arg(&out_base)
        .arg("--vad").arg("-vm").arg(&whisper.vad) // ignore les passages sans voix
        .arg("-sns"); // supprime les jetons « non vocaux » (♪, [Musique]…)
    tools::run_streaming(cmd, "la reconnaissance des paroles (whisper)", &mut |line| {
        if line.contains("progress") {
            if let Some(p) = tools::parse_percent(line) {
                progress(p);
            }
        }
    })?;
    let _ = std::fs::remove_file(&wav);
    read_whisper(&json, language)
}

/// Lit le résultat brut de Whisper : (langue, mots horodatés).
pub fn read_whisper(json: &Path, language: &str) -> Result<(String, Vec<Word>)> {
    // Le JSON de whisper.cpp peut contenir des octets UTF-8 tronqués : lecture tolérante.
    let bytes = std::fs::read(json).with_context(|| format!("résultat de whisper introuvable : {}", json.display()))?;
    let value: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&bytes))
        .context("résultat de whisper illisible")?;
    let language = value["result"]["language"].as_str().unwrap_or(language).to_string();

    let mut words: Vec<Word> = Vec::new();
    for seg in value["transcription"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let text = seg["text"].as_str().unwrap_or("").trim().to_string();
        let start = seg["offsets"]["from"].as_f64().unwrap_or(0.0) / 1000.0;
        let end = seg["offsets"]["to"].as_f64().unwrap_or(0.0) / 1000.0;
        push_word(&mut words, text, start, end);
    }
    if words.is_empty() {
        bail!("aucune parole reconnue");
    }
    Ok((language, words))
}

/// Paroles exactes saisies (une ligne par vers ; les horodatages LRC « [mm:ss.xx] »
/// sont reconnus s'ils sont présents).
pub fn parse_text(text: &str) -> Vec<crate::align::TextLine> {
    use crate::align::TextLine;
    let synced = crate::lrclib::parse_lrc(text);
    if synced.len() >= 2 {
        return synced.into_iter().map(|(t, s)| TextLine { start: Some(t), text: s }).collect();
    }
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| TextLine { start: None, text: l.to_string() })
        .collect()
}

/// Ajoute un mot en filtrant les annotations ([Musique], ♪…) et en rattachant la
/// ponctuation isolée au mot précédent.
fn push_word(words: &mut Vec<Word>, text: String, start: f64, end: f64) {
    if text.is_empty() {
        return;
    }
    let bracketed = (text.starts_with('[') && text.ends_with(']')) || (text.starts_with('(') && text.ends_with(')'));
    if bracketed || text.contains('♪') || text.contains('�') {
        return;
    }
    if !text.chars().any(char::is_alphanumeric) {
        if let Some(prev) = words.last_mut() {
            prev.text.push_str(&text);
            prev.end = prev.end.max(end);
        }
        return;
    }
    let end = if end > start { end } else { start + 0.2 };
    words.push(Word { text, start, end });
}

/// Regroupe les mots en lignes lisibles (pause, ponctuation finale, longueur).
pub fn group_lines(words: Vec<Word>) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut cur: Vec<Word> = Vec::new();
    for w in words {
        if let Some(prev) = cur.last() {
            let gap = w.start - prev.end;
            let chars: usize = cur.iter().map(|w| w.text.chars().count() + 1).sum();
            let ends_sentence = prev.text.ends_with(['.', '!', '?', '…']);
            let ends_clause = prev.text.ends_with([',', ';', ':']) && cur.len() >= 4;
            if gap > 1.0 || ends_sentence || ends_clause || cur.len() >= 9 || chars + w.text.len() > 42 {
                lines.push(Line { words: std::mem::take(&mut cur) });
            }
        }
        cur.push(w);
    }
    if !cur.is_empty() {
        lines.push(Line { words: cur });
    }
    lines
}

/// Sous-titres karaoké au format ASS (balises `\kf` : remplissage progressif des mots).
/// `tempo` : vitesse de lecture appliquée (les temps sont divisés d'autant).
/// Nom de famille interne de `assets/fonts/Nunito-ExtraBold.ttf` (fourni à libass
/// lors de l'export) : c'est ce nom qu'il faut citer pour que libass trouve la police.
pub const ASS_FONT: &str = "Nunito ExtraLight ExtraBold";

/// `highlight` : couleur des mots chantés (RVB).
pub fn to_ass(lyrics: &Lyrics, tempo: f64, width: u32, height: u32, highlight: [u8; 3]) -> String {
    let lyrics = &lyrics.shifted();
    // Couleurs ASS : &HAABBGGRR
    let primary = format!("&H00{:02X}{:02X}{:02X}", highlight[2], highlight[1], highlight[0]);
    let font_size = (height as f64 * 0.075).round();
    let margin_v = (height as f64 * 0.07).round();
    let mut s = String::new();
    let _ = write!(
        s,
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {width}\nPlayResY: {height}\nWrapStyle: 2\nScaledBorderAndShadow: yes\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, \
         Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Karaoke,{ASS_FONT},{font_size},{primary},&H00FFFFFF,&H00241C25,&H80000000,-1,0,0,0,100,100,0,0,1,{outline},2,2,40,40,{margin_v},1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        outline = (font_size / 14.0).max(2.0).round(),
    );
    let lines = &lyrics.lines;
    for (i, line) in lines.iter().enumerate() {
        let lead = 0.8;
        let show = (line.start() - lead).max(i.checked_sub(1).map_or(0.0, |p| lines[p].end()));
        let mut hide = line.end() + 0.5;
        if let Some(next) = lines.get(i + 1) {
            hide = hide.min((next.start() - lead).max(line.end()));
        }
        let mut text = String::new();
        let mut cursor = show;
        for w in &line.words {
            let gap = ((w.start - cursor) * 100.0 / tempo).round().max(0.0) as i64;
            if gap > 0 {
                let _ = write!(text, "{{\\k{gap}}}");
            }
            let dur = ((w.end - w.start.max(cursor)) * 100.0 / tempo).round().max(1.0) as i64;
            let _ = write!(text, "{{\\kf{dur}}}{} ", ass_escape(&w.text));
            cursor = w.end.max(cursor);
        }
        let _ = writeln!(
            s,
            "Dialogue: 0,{},{},Karaoke,,0,0,0,,{}",
            ass_time(show / tempo),
            ass_time(hide / tempo),
            text.trim_end()
        );
    }
    s
}

fn ass_escape(t: &str) -> String {
    t.replace('{', "(").replace('}', ")").replace('\\', "/")
}

fn ass_time(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, start: f64, end: f64) -> Word {
        Word { text: text.into(), start, end }
    }

    #[test]
    fn filtrage_et_regroupement() {
        let mut words = Vec::new();
        for (t, s, e) in [
            ("[Musique]", 0.0, 2.0),
            ("un", 2.0, 2.3),
            ("deux", 2.3, 2.6),
            (",", 2.6, 2.6),
            ("♪", 2.7, 2.8),
            ("trois.", 2.8, 3.1),
            ("quatre", 6.0, 6.4),
        ] {
            push_word(&mut words, t.into(), s, e);
        }
        let texts: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["un", "deux,", "trois.", "quatre"]);
        let lines = group_lines(words);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text(), "un deux, trois.");
    }

    #[test]
    fn ass_karaoke() {
        let ly = Lyrics {
            language: "fr".into(),
            lines: vec![Line { words: vec![w("un", 1.0, 1.5), w("deux", 2.0, 2.5)] }],
            offset: 0.0, source: String::new(),
        };
        let ass = to_ass(&ly, 2.0, 1920, 1080, [0xf7, 0xa6, 0xc9]);
        assert!(ass.contains(&format!(",{ASS_FONT},81,&H00C9A6F7,")), "{ass}");
        // Vitesse ×2 : la ligne apparaît à (1.0 − 0.8) / 2 = 0,10 s.
        assert!(ass.contains("Dialogue: 0,0:00:00.10,"), "{ass}");
        assert!(ass.contains("{\\k40}{\\kf25}un {\\k25}{\\kf25}deux"), "{ass}");
        assert_eq!(ass_time(3725.5), "1:02:05.50");
    }

    #[test]
    fn ligne_courante() {
        let ly = Lyrics {
            language: "fr".into(),
            lines: vec![
                Line { words: vec![w("a", 10.0, 11.0)] },
                Line { words: vec![w("b", 20.0, 21.0)] },
            ],
            offset: 0.0, source: String::new(),
        };
        assert_eq!(ly.line_at(0.0), None);
        assert_eq!(ly.line_at(8.0), Some(0));
        assert_eq!(ly.line_at(11.2), Some(0));
        assert_eq!(ly.line_at(15.0), None);
        assert_eq!(ly.line_at(18.0), Some(1));
        assert_eq!(ly.line_at(30.0), None);
    }
}
