//! Recherche des paroles officielles dans LRCLIB (https://lrclib.net), base de données
//! libre et ouverte de paroles synchronisées (format LRC, une ligne horodatée par vers).

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    track_name: Option<String>,
    artist_name: Option<String>,
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

/// Paroles trouvées en ligne.
pub enum Found {
    /// Vers horodatés : (début en secondes, texte). Un texte vide marque une pause.
    Synced(Vec<(f64, String)>),
    /// Vers sans horodatage.
    Plain(Vec<String>),
}

pub struct Match {
    pub description: String,
    pub lyrics: Found,
}

const USER_AGENT: &str = "Karaoke/0.1 (application de bureau, https://github.com/)";

/// Cherche les paroles d'un morceau. `title` : titre YouTube ou nom de fichier ;
/// `artist` / `track` : métadonnées si connues ; `duration` : durée en secondes.
pub fn find(title: &str, artist: Option<&str>, track: Option<&str>, duration: f64) -> Result<Option<Match>> {
    let mut queries = Vec::new();
    if let (Some(a), Some(t)) = (artist, track) {
        queries.push(format!("{} {}", clean_title(t), a));
    }
    queries.push(clean_title(title));

    let agent = ureq::Agent::new_with_defaults();
    for q in queries.iter().filter(|q| !q.trim().is_empty()) {
        let body = agent
            .get("https://lrclib.net/api/search")
            .query("q", q)
            .header("User-Agent", USER_AGENT)
            .call()
            .context("recherche LRCLIB")?
            .body_mut()
            .read_to_string()
            .context("réponse LRCLIB illisible")?;
        let entries: Vec<Entry> = serde_json::from_str(&body).context("réponse LRCLIB illisible")?;
        if let Some(m) = best(entries, duration, q) {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// Choisit la meilleure entrée : durée proche, paroles synchronisées de préférence,
/// titre ressemblant à la recherche.
fn best(entries: Vec<Entry>, duration: f64, query: &str) -> Option<Match> {
    let q = words(query);
    let mut scored: Vec<(f64, Entry)> = entries
        .into_iter()
        .filter(|e| !e.instrumental && (e.synced_lyrics.is_some() || e.plain_lyrics.is_some()))
        .filter_map(|e| {
            let dt = e.duration.map_or(99.0, |d| (d - duration).abs());
            if dt > 8.0 {
                return None;
            }
            let name = format!("{} {}", e.track_name.as_deref().unwrap_or(""), e.artist_name.as_deref().unwrap_or(""));
            let n = words(&name);
            let common = q.iter().filter(|w| n.contains(w)).count() as f64 / q.len().max(1) as f64;
            if common < 0.5 {
                return None;
            }
            let score = common * 10.0 - dt + if e.synced_lyrics.is_some() { 5.0 } else { 0.0 };
            Some((score, e))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (_, e) = scored.into_iter().next()?;
    let description = format!(
        "{} — {}",
        e.artist_name.as_deref().unwrap_or("?"),
        e.track_name.as_deref().unwrap_or("?")
    );
    let lyrics = match e.synced_lyrics.as_deref().map(parse_lrc).filter(|l| l.len() >= 2) {
        Some(lines) => Found::Synced(lines),
        None => Found::Plain(
            e.plain_lyrics
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
        ),
    };
    Some(Match { description, lyrics })
}

/// Lit un texte LRC (« [mm:ss.xx] texte »).
pub fn parse_lrc(lrc: &str) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    for line in lrc.lines() {
        let mut rest = line.trim();
        let mut times = Vec::new();
        // Une ligne peut porter plusieurs horodatages : [00:12.00][01:30.50] texte
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            rest = stripped[end + 1..].trim_start();
            if let Some((m, s)) = tag.split_once(':') {
                if let (Ok(m), Ok(s)) = (m.trim().parse::<f64>(), s.trim().parse::<f64>()) {
                    times.push(m * 60.0 + s);
                }
            }
        }
        for t in times {
            out.push((t, rest.trim().to_string()));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Retire du titre YouTube ce qui gêne la recherche : (Official Video), [Lyrics],
/// | NCS Release, feat. …
pub fn clean_title(title: &str) -> String {
    let mut s = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' | '【' => depth += 1,
            ')' | ']' | '】' => depth = (depth - 1).max(0),
            _ if depth == 0 => s.push(c),
            _ => {}
        }
    }
    let s = s.split(['|', '｜']).next().unwrap_or("").to_string();
    let lower = s.to_lowercase();
    let cut = [" feat.", " feat ", " ft.", " ft ", " official", " lyrics", " clip officiel", " audio"]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        .unwrap_or(s.len());
    s[..cut].replace([',', '-', '–', '"', '“', '”'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nettoyage_titre() {
        assert_eq!(
            clean_title("Cartoon, Jéja - On & On (feat. Daniel Levi) | Electronic Pop | NCS - Copyright Free Music"),
            "Cartoon Jéja On & On"
        );
        assert_eq!(clean_title("Artiste - Titre [Official Video]"), "Artiste Titre");
        assert_eq!(clean_title("Artiste - Titre ft. Autre"), "Artiste Titre");
    }

    #[test]
    fn lecture_lrc() {
        let l = parse_lrc("[00:01.50] a\n[ti:titre]\n[01:02.00][00:10.00] b\n[00:20.00]\n");
        let t: Vec<_> = l.iter().map(|(t, s)| (*t, s.as_str())).collect();
        assert_eq!(t, [(1.5, "a"), (10.0, "b"), (20.0, ""), (62.0, "b")]);
    }
}
