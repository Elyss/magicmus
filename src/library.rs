//! Bibliothèque (chansons rangées en dossiers) et historique, enregistrés dans
//! `Documents\MagicMus` avec les chansons elles-mêmes (`Chansons\`).

use anyhow::Result;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Dossier de l'utilisateur : `Documents\MagicMus` (ou `MAGICMUS_DATA` si défini :
/// version portable, tests).
pub fn data_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os("MAGICMUS_DATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(custom);
    }
    user_folder("Documents").join("MagicMus")
}

/// Dossier personnel de l'utilisateur : `"Documents"` ou `"Music"`.
///
/// L'emplacement réel (qui peut être redirigé, vers OneDrive par exemple) est lu dans
/// le registre, là où l'Explorateur le range (« User Shell Folders ») ; à défaut,
/// `USERPROFILE\<nom>`. Note : la caisse `dirs` faisait planter l'exe graphique au
/// démarrage (avant `main`) ; elle n'est donc pas utilisée.
pub fn user_folder(name: &str) -> PathBuf {
    let value = match name {
        "Documents" => Some("Personal"),
        "Music" => Some("My Music"),
        _ => None,
    };
    value
        .and_then(registry_shell_folder)
        .unwrap_or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(name)
        })
}

/// Lit `HKCU\…\Explorer\User Shell Folders\<value>` (variables `%…%` développées).
fn registry_shell_folder(value: &str) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let out = crate::tools::command("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders", "/v", value])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.trim_start().starts_with(value))?;
    let raw = line.split("REG_").nth(1)?.split_once(char::is_whitespace)?.1.trim();
    let path = PathBuf::from(expand_env(raw));
    path.is_dir().then_some(path)
}

/// Développe les variables d'environnement `%NOM%`.
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Dossier des chansons traitées.
pub fn songs_dir() -> PathBuf {
    data_dir().join("Chansons")
}

/// Heure actuelle (secondes Unix).
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Lecture tolérante d'un JSON (absent ou illisible → valeur par défaut).
pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| {
            let b = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&b);
            serde_json::from_slice(b).ok()
        })
        .unwrap_or_default()
}

/// Écriture sûre : fichier temporaire puis renommage (jamais de fichier à moitié écrit).
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Titre d'une chanson d'après son dossier (`titre.txt`), sinon le nom du dossier.
pub fn song_title(karaoke: &Path) -> String {
    let dir = karaoke.parent().unwrap_or(Path::new("."));
    std::fs::read_to_string(dir.join("titre.txt"))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned())
}

/// Fichier karaoké d'un dossier de chanson (le plus récent si plusieurs).
pub fn karaoke_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("karaoke_"))
                && p.extension().is_some_and(|e| e == "mp4" || e == "m4a")
        })
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

fn same(a: &Path, b: &Path) -> bool {
    a == b || a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

// ───────────────────────────── Bibliothèque ─────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Folder {
    pub id: u64,
    pub name: String,
    /// Dossier parent (`None` : à la racine).
    #[serde(default)]
    pub parent: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibSong {
    /// Fichier karaoké.
    pub path: PathBuf,
    pub title: String,
    /// Dossier de rangement (`None` : à la racine).
    #[serde(default)]
    pub folder: Option<u64>,
    pub added: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub folders: Vec<Folder>,
    pub songs: Vec<LibSong>,
}

impl Library {
    pub const FILE: &'static str = "bibliotheque.json";

    pub fn load(dir: &Path) -> Self {
        load_json(&dir.join(Self::FILE))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        save_json(&dir.join(Self::FILE), self)
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.songs.iter().any(|s| same(&s.path, path))
    }

    /// Ajoute une chanson (sans doublon) dans `folder`.
    pub fn add(&mut self, path: &Path, title: &str, folder: Option<u64>) {
        if let Some(s) = self.songs.iter_mut().find(|s| same(&s.path, path)) {
            s.folder = folder;
            return;
        }
        self.songs.push(LibSong { path: path.to_path_buf(), title: title.to_string(), folder, added: now() });
    }

    pub fn remove(&mut self, path: &Path) {
        self.songs.retain(|s| !same(&s.path, path));
    }

    pub fn move_song(&mut self, path: &Path, folder: Option<u64>) {
        if let Some(s) = self.songs.iter_mut().find(|s| same(&s.path, path)) {
            s.folder = folder;
        }
    }

    pub fn create_folder(&mut self, name: &str, parent: Option<u64>) -> u64 {
        let id = self.folders.iter().map(|f| f.id).max().unwrap_or(0) + 1;
        let name = if name.trim().is_empty() { "Nouveau dossier" } else { name.trim() };
        self.folders.push(Folder { id, name: name.to_string(), parent });
        id
    }

    pub fn rename_folder(&mut self, id: u64, name: &str) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            if !name.trim().is_empty() {
                f.name = name.trim().to_string();
            }
        }
    }

    /// Supprime un dossier : ses chansons et sous-dossiers remontent d'un niveau.
    pub fn delete_folder(&mut self, id: u64) {
        let Some(parent) = self.folders.iter().find(|f| f.id == id).map(|f| f.parent) else { return };
        for f in self.folders.iter_mut().filter(|f| f.parent == Some(id)) {
            f.parent = parent;
        }
        for s in self.songs.iter_mut().filter(|s| s.folder == Some(id)) {
            s.folder = parent;
        }
        self.folders.retain(|f| f.id != id);
    }

    /// Déplace un dossier (refusé s'il serait rangé dans l'un de ses descendants).
    pub fn move_folder(&mut self, id: u64, parent: Option<u64>) -> bool {
        if parent.is_some_and(|p| p == id || self.is_descendant(p, id)) {
            return false;
        }
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.parent = parent;
            return true;
        }
        false
    }

    /// `a` est-il (indirectement) dans `b` ?
    pub fn is_descendant(&self, a: u64, b: u64) -> bool {
        let mut cur = self.folders.iter().find(|f| f.id == a).and_then(|f| f.parent);
        let mut guard = 0;
        while let Some(c) = cur {
            if c == b {
                return true;
            }
            cur = self.folders.iter().find(|f| f.id == c).and_then(|f| f.parent);
            guard += 1;
            if guard > 1000 {
                break;
            }
        }
        false
    }

    /// Sous-dossiers directs, triés par nom.
    pub fn children(&self, parent: Option<u64>) -> Vec<&Folder> {
        let mut v: Vec<&Folder> = self.folders.iter().filter(|f| f.parent == parent).collect();
        v.sort_by_key(|f| f.name.to_lowercase());
        v
    }

    /// Chansons d'un dossier, triées par titre.
    pub fn songs_in(&self, folder: Option<u64>) -> Vec<&LibSong> {
        let mut v: Vec<&LibSong> = self.songs.iter().filter(|s| s.folder == folder).collect();
        v.sort_by_key(|s| s.title.to_lowercase());
        v
    }

    /// Nombre de chansons dans un dossier et ses sous-dossiers.
    pub fn count_in(&self, id: u64) -> usize {
        self.songs.iter().filter(|s| s.folder.is_some_and(|f| f == id || self.is_descendant(f, id))).count()
    }

    /// Chemin lisible d'un dossier (« Soirées / Années 2000 »).
    pub fn folder_path(&self, id: u64) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            match self.folders.iter().find(|f| f.id == c) {
                Some(f) => {
                    parts.push(f.name.clone());
                    cur = f.parent;
                }
                None => break,
            }
            if parts.len() > 50 {
                break;
            }
        }
        parts.reverse();
        parts.join(" / ")
    }

    /// Tous les dossiers, dans l'ordre de l'arborescence, avec leur profondeur.
    pub fn tree(&self) -> Vec<(usize, &Folder)> {
        let mut out = Vec::new();
        fn walk<'a>(lib: &'a Library, parent: Option<u64>, depth: usize, out: &mut Vec<(usize, &'a Folder)>) {
            for f in lib.children(parent) {
                out.push((depth, f));
                if depth < 20 {
                    walk(lib, Some(f.id), depth + 1, out);
                }
            }
        }
        walk(self, None, 0, &mut out);
        out
    }
}

// ───────────────────────────── Historique ─────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Fichier karaoké.
    pub path: PathBuf,
    pub title: String,
    /// Date de création du karaoké.
    pub created: i64,
    /// Dernière écoute dans le lecteur.
    #[serde(default)]
    pub last_played: Option<i64>,
    #[serde(default)]
    pub plays: u32,
    /// Adresse ou fichier d'origine.
    #[serde(default)]
    pub input: String,
}

impl HistoryEntry {
    /// Dernière activité (création ou écoute).
    pub fn last_activity(&self) -> i64 {
        self.last_played.unwrap_or(self.created).max(self.created)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    pub const FILE: &'static str = "historique.json";

    pub fn load(dir: &Path) -> Self {
        load_json(&dir.join(Self::FILE))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        save_json(&dir.join(Self::FILE), self)
    }

    fn entry(&mut self, path: &Path, title: &str, created: i64) -> &mut HistoryEntry {
        if let Some(i) = self.entries.iter().position(|e| same(&e.path, path)) {
            let e = &mut self.entries[i];
            if !title.is_empty() {
                e.title = title.to_string();
            }
            return e;
        }
        self.entries.push(HistoryEntry {
            path: path.to_path_buf(),
            title: title.to_string(),
            created,
            last_played: None,
            plays: 0,
            input: String::new(),
        });
        self.entries.last_mut().unwrap()
    }

    /// Karaoké créé (ou recréé) à l'instant `t`.
    pub fn record_created(&mut self, path: &Path, title: &str, input: &str, t: i64) {
        let e = self.entry(path, title, t);
        e.created = t;
        if !input.is_empty() {
            e.input = input.to_string();
        }
    }

    /// Karaoké ouvert dans le lecteur à l'instant `t`.
    pub fn record_played(&mut self, path: &Path, title: &str, t: i64) {
        let created = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|m| {
            m.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
        });
        let e = self.entry(path, title, created.unwrap_or(t));
        e.last_played = Some(t);
        e.plays += 1;
    }

    pub fn remove(&mut self, path: &Path) {
        self.entries.retain(|e| !same(&e.path, path));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Entrées de la plus récente à la plus ancienne activité.
    pub fn sorted(&self) -> Vec<&HistoryEntry> {
        let mut v: Vec<&HistoryEntry> = self.entries.iter().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.last_activity()));
        v
    }

    /// Ajoute les chansons présentes dans `songs` mais absentes de l'historique
    /// (chansons déplacées depuis l'ancien dossier, par exemple). Renvoie le nombre ajouté.
    pub fn scan(&mut self, songs: &Path) -> usize {
        let Ok(rd) = std::fs::read_dir(songs) else { return 0 };
        let mut added = 0;
        for dir in rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()) {
            let Some(k) = karaoke_file(&dir) else { continue };
            if self.entries.iter().any(|e| same(&e.path, &k)) {
                continue;
            }
            let created = std::fs::metadata(&k)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            let title = song_title(&k);
            self.entries.push(HistoryEntry { path: k, title, created, last_played: None, plays: 0, input: String::new() });
            added += 1;
        }
        added
    }
}

// ───────────────────────────── Ancien dossier ─────────────────────────────

/// Dossiers de chansons présents dans un ancien emplacement (ex. `Musique\Karaoke`).
pub fn song_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_dir() && karaoke_file(p).is_some())
                .collect()
        })
        .unwrap_or_default()
}

/// Déplace des dossiers de chansons dans `dest` (renommage, sinon copie puis
/// suppression). Les dossiers déjà présents à destination sont laissés en place.
/// Renvoie le nombre de dossiers déplacés.
pub fn move_song_dirs(dirs: &[PathBuf], dest: &Path) -> Result<usize> {
    std::fs::create_dir_all(dest)?;
    let mut moved = 0;
    for src in dirs {
        let Some(name) = src.file_name() else { continue };
        let target = dest.join(name);
        if target.exists() {
            continue;
        }
        if std::fs::rename(src, &target).is_err() {
            copy_dir(src, &target)?;
            std::fs::remove_dir_all(src)?;
        }
        // Le chemin de la source mémorisé pour les fichiers locaux reste valable ;
        // celui d'une source téléchargée suit le dossier.
        let source_txt = target.join("source.txt");
        if let Ok(old) = std::fs::read_to_string(&source_txt) {
            let old = PathBuf::from(old.trim());
            if old.starts_with(src) {
                if let Ok(rel) = old.strip_prefix(src) {
                    let _ = std::fs::write(&source_txt, target.join(rel).to_string_lossy().as_bytes());
                }
            }
        }
        moved += 1;
    }
    Ok(moved)
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &dst.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, dst.join(e.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn dossiers_et_rangement() {
        let mut lib = Library::default();
        let soirees = lib.create_folder("Soirées", None);
        let annees = lib.create_folder("Années 2000", Some(soirees));
        lib.add(&p("a/karaoke_ia.mp4"), "Zèbre", Some(annees));
        lib.add(&p("b/karaoke_ia.mp4"), "Abeille", None);
        lib.add(&p("b/karaoke_ia.mp4"), "Abeille", Some(soirees)); // pas de doublon : rangement
        assert_eq!(lib.songs.len(), 2);
        assert_eq!(lib.folder_path(annees), "Soirées / Années 2000");
        assert_eq!(lib.count_in(soirees), 2);
        assert!(lib.is_descendant(annees, soirees));
        assert!(!lib.move_folder(soirees, Some(annees)), "un dossier ne peut pas aller dans son sous-dossier");

        // Suppression : contenu remonté d'un niveau.
        lib.delete_folder(soirees);
        assert_eq!(lib.folders.len(), 1);
        assert_eq!(lib.folders[0].parent, None);
        assert_eq!(lib.songs_in(None).len(), 1);
        lib.remove(&p("a/karaoke_ia.mp4"));
        assert_eq!(lib.songs.len(), 1);
        assert_eq!(lib.tree().len(), 1);
    }

    #[test]
    fn variables_et_dossiers() {
        unsafe { std::env::set_var("MAGICMUS_TEST_VAR", "C:\\Exemple") };
        assert_eq!(expand_env("%MAGICMUS_TEST_VAR%\\Documents"), "C:\\Exemple\\Documents");
        assert_eq!(expand_env("%INCONNUE_XYZ%\\a"), "%INCONNUE_XYZ%\\a");
        assert_eq!(expand_env("100%"), "100%");
        // Le dossier Documents existe (registre ou USERPROFILE).
        assert!(user_folder("Documents").is_dir());
    }

    #[test]
    fn historique() {
        let mut h = History::default();
        h.record_created(&p("x/karaoke_ia.mp4"), "X", "https://exemple", 100);
        h.record_created(&p("y/karaoke_ia.mp4"), "Y", "", 200);
        h.record_played(&p("x/karaoke_ia.mp4"), "X", 300);
        let order: Vec<&str> = h.sorted().iter().map(|e| e.title.as_str()).collect();
        assert_eq!(order, ["X", "Y"]);
        assert_eq!(h.entries[0].plays, 1);
        assert_eq!(h.entries.len(), 2);
        h.remove(&p("y/karaoke_ia.mp4"));
        assert_eq!(h.entries.len(), 1);
    }

    #[test]
    fn sauvegarde_et_relecture() {
        let dir = std::env::temp_dir().join(format!("magicmus-test-{}", std::process::id()));
        let mut lib = Library::default();
        let f = lib.create_folder("Duo", None);
        lib.add(&p("c/karaoke_ia.mp4"), "C", Some(f));
        lib.save(&dir).unwrap();
        let back = Library::load(&dir);
        assert_eq!(back.songs.len(), 1);
        assert_eq!(back.songs[0].folder, Some(f));
        assert!(Library::load(&dir.join("absent")).songs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
