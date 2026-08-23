//! Presets: saved pipelines, one TOML file each.
//!
//! **D8** — *"presets are just saved pipelines"* — and **D18** — the job-file
//! schema *is* the preset schema. So there is no preset *format* here, only a
//! preset *policy*: a preset names itself, never names a source, and may be
//! empty. Everything about reading and writing the file lives in [`crate::job`].
//!
//! The folder is the database. No index, no settings file: `list()` reads the
//! directory, import and export are file copies, and a preset a user drops in
//! by hand works exactly like one the app wrote. That also keeps
//! `app_data_dir` down to two callers — the journal and this — and leaves the
//! whole settings-file question to M8, where the roadmap already puts it.

use std::path::{Path, PathBuf};

use ren_platform::naming;

use crate::job::{Document, JobError, PresetMeta, Source};
use crate::ops::OpKind;
use crate::pipeline::{Pipeline, StepConfig};
use crate::run::{Answers, AskSpec, RunSettings};

/// A saved pipeline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Preset {
    pub name: String,
    pub description: String,
    pub steps: Vec<(OpKind, StepConfig)>,
    pub settings: RunSettings,
}

/// What loading an outside file quietly discarded.
///
/// Importing a job file as a preset is meant to work — it is the best thing
/// about sharing one schema — but the source it names is not the user's, so it
/// is dropped and reported rather than silently obeyed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportNotes {
    pub dropped_source: Option<Source>,
}

impl ImportNotes {
    pub fn is_empty(&self) -> bool {
        self.dropped_source.is_none()
    }
}

impl Preset {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn parse(text: &str) -> Result<(Self, ImportNotes), JobError> {
        Ok(Self::from_document(Document::parse(text)?))
    }

    pub fn to_toml(&self) -> Result<String, JobError> {
        self.to_document().to_toml()
    }

    pub fn from_document(doc: Document) -> (Self, ImportNotes) {
        let notes = ImportNotes {
            dropped_source: doc.source,
        };
        let preset = Self {
            name: doc.meta.name,
            description: doc.meta.description,
            steps: doc.steps,
            settings: doc.settings,
        };
        (preset, notes)
    }

    /// Always without a `[source]`: a preset runs over whatever the caller is
    /// looking at.
    pub fn to_document(&self) -> Document {
        Document {
            meta: PresetMeta {
                name: self.name.clone(),
                description: self.description.clone(),
            },
            source: None,
            steps: self.steps.clone(),
            settings: RunSettings {
                // A preset never carries a seed. Zero means "nobody has chosen
                // one" (P16), and the front ends pick a fresh one per session
                // — so writing the session's seed into a preset would freeze
                // `<Rnd*>` at whatever value happened to be live when the user
                // pressed Save, and every later run of that preset would
                // produce the same "random" names. Pinning a seed deliberately
                // is a *job file's* job, which is where reproducibility lives.
                seed: 0,
                ..self.settings.clone()
            },
        }
    }

    pub fn pipeline(&self) -> Pipeline {
        self.pipeline_with(Answers::default())
    }

    pub fn pipeline_with(&self, answers: Answers) -> Pipeline {
        let mut pipeline = Pipeline::new();
        for (op, config) in &self.steps {
            pipeline.push(op.to_step(), config.clone());
        }
        pipeline.settings = self.settings.clone();
        pipeline.answers = answers;
        pipeline
    }

    /// Every `<Ask>` slot the preset will want answered.
    pub fn asks(&self) -> Vec<AskSpec> {
        self.pipeline().asks()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// One line for the drawer.
    pub fn summary(&self) -> String {
        match self.steps.len() {
            0 => "no operations".to_owned(),
            1 => "1 operation".to_owned(),
            n => format!("{n} operations"),
        }
    }
}

/// `<per-user data dir>/RenameIt/presets` — beside the journal (D16).
pub fn default_preset_dir() -> PathBuf {
    ren_platform::app_data_dir("RenameIt").join("presets")
}

/// One preset in the folder, as the drawer lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetEntry {
    /// `[preset].name`, falling back to the file stem for a hand-dropped file.
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub steps: usize,
}

/// A file in the folder that is not a readable preset.
///
/// Surfaced rather than hidden: a preset that stopped loading is exactly what
/// the user needs told, and a folder with one bad file in it must still open.
#[derive(Debug)]
pub struct PresetProblem {
    pub path: PathBuf,
    pub error: PresetError,
}

#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Parse(#[from] JobError),
    #[error("a preset needs a name")]
    EmptyName,
    #[error("no preset named {name:?} in {dir}{known}")]
    NotFound {
        name: String,
        dir: PathBuf,
        known: String,
    },
    #[error("{name:?} matches {count} presets: {paths}")]
    Ambiguous {
        name: String,
        count: usize,
        paths: String,
    },
    #[error("{0} is not in the preset folder")]
    Outside(PathBuf),
}

/// A directory of `*.toml` presets.
#[derive(Debug, Clone)]
pub struct PresetStore {
    dir: PathBuf,
}

impl PresetStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The per-user folder.
    pub fn user() -> Self {
        Self::new(default_preset_dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every readable preset, by display name, plus whatever would not load.
    ///
    /// A folder that does not exist yet is an empty list, not an error — the
    /// same rule `Journal::list` follows, and a first run has no folder.
    pub fn list(&self) -> (Vec<PresetEntry>, Vec<PresetProblem>) {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return (Vec::new(), Vec::new());
        };

        let mut entries = Vec::new();
        let mut problems = Vec::new();
        for path in read.filter_map(Result::ok).map(|e| e.path()).filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
        }) {
            match self.load(&path) {
                Ok((preset, _)) => entries.push(PresetEntry {
                    name: display_name(&preset, &path),
                    description: preset.description.clone(),
                    steps: preset.steps.len(),
                    path,
                }),
                Err(error) => problems.push(PresetProblem { path, error }),
            }
        }
        entries.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        });
        (entries, problems)
    }

    pub fn load(&self, path: &Path) -> Result<(Preset, ImportNotes), PresetError> {
        let text = std::fs::read_to_string(path).map_err(|source| PresetError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let (mut preset, notes) = Preset::parse(&text)?;
        if preset.name.is_empty() {
            preset.name = stem_of(path);
        }
        Ok((preset, notes))
    }

    /// By display name, the way `/r "preset name"` works.
    pub fn load_named(&self, name: &str) -> Result<Preset, PresetError> {
        let matches = self.find_by_name(name);
        match matches.len() {
            1 => self.load(&matches[0].path).map(|(preset, _)| preset),
            0 => {
                let (all, _) = self.list();
                let known = if all.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (found: {})",
                        all.iter()
                            .map(|e| e.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                Err(PresetError::NotFound {
                    name: name.to_owned(),
                    dir: self.dir.clone(),
                    known,
                })
            }
            // Two presets may legitimately share a display name; guessing which
            // one the user meant is how the wrong pipeline runs over a folder.
            count => Err(PresetError::Ambiguous {
                name: name.to_owned(),
                count,
                paths: matches
                    .iter()
                    .map(|e| e.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            }),
        }
    }

    pub fn find_by_name(&self, name: &str) -> Vec<PresetEntry> {
        let (entries, _) = self.list();
        entries
            .into_iter()
            .filter(|e| e.name.eq_ignore_ascii_case(name))
            .collect()
    }

    /// Writes the preset, returning where it went.
    ///
    /// Overwrites the file that already holds a preset of this name — that is
    /// "save over the one you loaded" — and otherwise suffixes ` (2)`.
    pub fn save(&self, preset: &Preset) -> Result<PathBuf, PresetError> {
        if preset.name.trim().is_empty() {
            return Err(PresetError::EmptyName);
        }
        let path = self.path_for(preset)?;
        self.save_as(preset, &path)?;
        Ok(path)
    }

    pub fn save_as(&self, preset: &Preset, path: &Path) -> Result<(), PresetError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| PresetError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_atomically(path, &preset.to_toml()?)
    }

    pub fn delete(&self, path: &Path) -> Result<(), PresetError> {
        self.must_be_ours(path)?;
        std::fs::remove_file(path).map_err(|source| PresetError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Renames both the file and the name inside it, so the two cannot drift.
    pub fn rename(&self, path: &Path, new_name: &str) -> Result<PathBuf, PresetError> {
        self.must_be_ours(path)?;
        if new_name.trim().is_empty() {
            return Err(PresetError::EmptyName);
        }
        let (mut preset, _) = self.load(path)?;
        preset.name = new_name.to_owned();
        let target = self.path_for(&preset)?;
        self.save_as(&preset, &target)?;
        if target != path {
            let _ = std::fs::remove_file(path);
        }
        Ok(target)
    }

    pub fn duplicate(&self, path: &Path) -> Result<PathBuf, PresetError> {
        self.must_be_ours(path)?;
        let (mut preset, _) = self.load(path)?;
        preset.name = format!("{} (copy)", preset.name);
        self.save(&preset)
    }

    /// Copies an outside file in, keeping whatever it can.
    pub fn import(&self, from: &Path) -> Result<(PathBuf, ImportNotes), PresetError> {
        let (mut preset, notes) = self.load(from)?;
        if preset.name.trim().is_empty() {
            preset.name = stem_of(from);
        }
        Ok((self.save(&preset)?, notes))
    }

    pub fn export(&self, preset: &Preset, to: &Path) -> Result<(), PresetError> {
        self.save_as(preset, to)
    }

    /// The file this preset belongs in.
    /// Writes the six presets a fresh install starts with, and returns how
    /// many it wrote.
    ///
    /// Six default presets ship with the app.
    ///
    /// **D6**: written as our own TOML rather than carried in some positional
    /// binary format nobody else will ever write.
    ///
    /// **Only into a folder that does not exist**, which is the one place this
    /// diverges from [`crate::script::ScriptStore::seed_defaults`] — and the
    /// divergence is the whole point. A script the user deleted is a file they
    /// stopped using; a **preset** the user deleted is a menu item they removed
    /// from their file manager's context menu, and putting it back on the next
    /// start is the app arguing with them about their own right-click menu.
    /// Seeding once, into nothing, cannot do that.
    pub fn seed_defaults(&self) -> Result<usize, PresetError> {
        if self.dir.exists() {
            return Ok(0);
        }
        std::fs::create_dir_all(&self.dir).map_err(|source| PresetError::Io {
            path: self.dir.clone(),
            source,
        })?;
        let mut written = 0;
        for (stem, source) in DEFAULTS {
            let path = self.dir.join(format!("{stem}.toml"));
            std::fs::write(&path, source).map_err(|source| PresetError::Io {
                path: path.clone(),
                source,
            })?;
            written += 1;
        }
        Ok(written)
    }

    fn path_for(&self, preset: &Preset) -> Result<PathBuf, PresetError> {
        let stem = file_stem_for(&preset.name);
        let first = self.dir.join(format!("{stem}.toml"));
        if !first.exists() {
            return Ok(first);
        }
        // Ours already? Then this is a save over the preset we loaded.
        if let Ok((existing, _)) = self.load(&first)
            && existing.name.eq_ignore_ascii_case(&preset.name)
        {
            return Ok(first);
        }
        for n in 2..1000 {
            let candidate = self.dir.join(format!("{stem} ({n}).toml"));
            if !candidate.exists() {
                return Ok(candidate);
            }
        }
        Ok(self.dir.join(format!("{stem} (999).toml")))
    }

    /// A path handed in by a UI list is not automatically ours to delete.
    fn must_be_ours(&self, path: &Path) -> Result<(), PresetError> {
        if path.parent() == Some(self.dir.as_path()) {
            Ok(())
        } else {
            Err(PresetError::Outside(path.to_path_buf()))
        }
    }
}

fn display_name(preset: &Preset, path: &Path) -> String {
    if preset.name.trim().is_empty() {
        stem_of(path)
    } else {
        preset.name.clone()
    }
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "preset".to_owned())
}

/// A display name turned into a file stem that works everywhere.
///
/// Sanitised against the **Windows** table on every platform, deliberately: a
/// preset folder should survive being copied from a Linux machine to a Windows
/// one, so the strictest rules win. `ren_platform::naming` already knows them.
pub fn file_stem_for(name: &str) -> String {
    let mut stem = String::with_capacity(name.len());
    let mut last_was_underscore = false;
    for c in name.trim().chars() {
        let safe = !naming::WINDOWS.illegal_chars.contains(&c) && !c.is_control();
        if safe {
            stem.push(c);
            last_was_underscore = false;
        } else if !last_was_underscore {
            stem.push('_');
            last_was_underscore = true;
        }
    }

    // Trailing dots and spaces are legal to type and impossible to store.
    let mut stem = stem.trim_end_matches(['.', ' ']).to_owned();

    // Leave room for " (999).toml" inside the 255-unit component limit.
    while stem.encode_utf16().count() > 96 {
        stem.pop();
    }
    let stem = stem.trim_end_matches(['.', ' ']).to_owned();

    if stem.is_empty() {
        return "preset".to_owned();
    }
    // CON, LPT1 and friends are reserved with or without an extension.
    if naming::WINDOWS
        .reserved_stems
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&stem))
    {
        return format!("{stem}_");
    }
    stem
}

/// Writes via a temporary file, so an interrupted save cannot truncate the
/// preset that was already there.
///
/// `std::fs::rename`, **not** `Platform::rename`: that one refuses to overwrite
/// by design (P13), which is right for a user's files and exactly wrong here.
/// This is our own data file, and replacing it is the whole point.
fn write_atomically(path: &Path, text: &str) -> Result<(), PresetError> {
    use std::io::Write;

    let temp = path.with_extension("toml.tmp");
    let io = |path: &Path, source: std::io::Error| PresetError::Io {
        path: path.to_path_buf(),
        source,
    };

    let mut file = std::fs::File::create(&temp).map_err(|e| io(&temp, e))?;
    file.write_all(text.as_bytes()).map_err(|e| io(&temp, e))?;
    file.sync_all().map_err(|e| io(&temp, e))?;
    drop(file);

    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        io(path, e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{Casing, Replace};
    use tempfile::TempDir;

    fn sample() -> Preset {
        Preset {
            name: "Photo cleanup".to_owned(),
            description: "Underscores out, title case.".to_owned(),
            steps: vec![
                (
                    OpKind::Replace(Replace::new("_", " ")),
                    StepConfig::default(),
                ),
                (
                    OpKind::Casing(Casing::new(crate::ops::CaseMode::Title)),
                    StepConfig::default(),
                ),
            ],
            settings: RunSettings::default(),
        }
    }

    #[test]
    fn a_preset_round_trips_through_its_file_format() {
        let preset = sample();
        let (back, notes) = Preset::parse(&preset.to_toml().unwrap()).unwrap();
        assert_eq!(back, preset);
        assert!(notes.is_empty());
    }

    /// A preset runs over whatever the caller is looking at, so it must never
    /// carry a folder of its own.
    #[test]
    fn a_preset_never_writes_a_source_section() {
        let text = sample().to_toml().unwrap();
        assert!(!text.contains("[source]"), "{text}");
    }

    /// Importing a job file is meant to work — same schema (D18) — but the
    /// folder it names is not the user's, so it is dropped and reported.
    #[test]
    fn importing_a_job_file_keeps_the_pipeline_and_reports_the_dropped_source() {
        let (preset, notes) = Preset::parse(
            r#"
            [source]
            dir = "/somewhere/else"

            [[step]]
            op = "space_trim"
        "#,
        )
        .unwrap();

        assert_eq!(preset.steps.len(), 1);
        assert_eq!(
            notes.dropped_source.unwrap().dir(),
            PathBuf::from("/somewhere/else")
        );
    }

    /// "presets persist and reload across restarts" — a second store over the
    /// same folder, carrying nothing over from the first.
    #[test]
    fn a_preset_saved_in_one_store_reloads_in_another() {
        let dir = TempDir::new().unwrap();
        let preset = sample();
        let path = PresetStore::new(dir.path()).save(&preset).unwrap();
        assert_eq!(path.parent(), Some(dir.path()));

        let reopened = PresetStore::new(dir.path());
        let (entries, problems) = reopened.list();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Photo cleanup");
        assert_eq!(entries[0].steps, 2);

        let (back, _) = reopened.load(&entries[0].path).unwrap();
        assert_eq!(back, preset);
        assert_eq!(reopened.load_named("Photo cleanup").unwrap(), preset);
    }

    #[test]
    fn a_missing_folder_lists_as_empty_rather_than_failing() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path().join("not-created-yet"));
        let (entries, problems) = store.list();
        assert!(entries.is_empty());
        assert!(problems.is_empty());

        // And saving creates it.
        store.save(&sample()).unwrap();
        assert_eq!(store.list().0.len(), 1);
    }

    /// One unreadable file must not take the whole drawer down with it.
    #[test]
    fn an_unreadable_preset_is_reported_and_the_others_still_list() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        store.save(&sample()).unwrap();
        std::fs::write(dir.path().join("broken.toml"), "this is not = toml [").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored entirely").unwrap();

        let (entries, problems) = store.list();
        assert_eq!(entries.len(), 1, "the good one still lists");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].path.ends_with("broken.toml"));
    }

    /// A file dropped in by hand has no `[preset]` table; its filename is its
    /// name.
    #[test]
    fn a_preset_with_no_name_falls_back_to_its_file_name() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Tidy up.toml"),
            "[[step]]\nop = \"space_trim\"\n",
        )
        .unwrap();
        let store = PresetStore::new(dir.path());
        assert_eq!(store.list().0[0].name, "Tidy up");
        assert!(store.load_named("Tidy up").is_ok());
    }

    #[test]
    fn saving_over_the_preset_you_loaded_replaces_it() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        let first = store.save(&sample()).unwrap();

        let mut edited = sample();
        edited.steps.pop();
        let second = store.save(&edited).unwrap();

        assert_eq!(first, second, "same name, same file");
        assert_eq!(store.list().0.len(), 1);
        assert_eq!(store.load(&second).unwrap().0.steps.len(), 1);
    }

    /// Two presets may share a display name. The files must not collide, and a
    /// lookup by name must refuse to guess.
    #[test]
    fn two_presets_with_one_name_get_two_files_and_an_ambiguous_lookup() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        store.save(&sample()).unwrap();
        // A second file with the same display name, written directly.
        std::fs::write(
            dir.path().join("elsewhere.toml"),
            "[preset]\nname = \"Photo cleanup\"\n",
        )
        .unwrap();

        assert_eq!(store.list().0.len(), 2);
        assert!(matches!(
            store.load_named("Photo cleanup"),
            Err(PresetError::Ambiguous { count: 2, .. })
        ));
    }

    #[test]
    fn an_unknown_name_lists_the_ones_that_exist() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        store.save(&sample()).unwrap();

        let err = store.load_named("Nope").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Nope"), "{message}");
        assert!(message.contains("Photo cleanup"), "{message}");
    }

    #[test]
    fn renaming_moves_the_file_and_rewrites_the_name_inside_it() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        let path = store.save(&sample()).unwrap();

        let moved = store.rename(&path, "Holiday photos").unwrap();
        assert!(!path.exists(), "the old file should be gone");
        assert_eq!(store.load(&moved).unwrap().0.name, "Holiday photos");
        assert_eq!(store.list().0.len(), 1);
    }

    #[test]
    fn duplicating_appends_copy_and_keeps_both() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        let path = store.save(&sample()).unwrap();

        let copy = store.duplicate(&path).unwrap();
        assert_ne!(copy, path);
        assert_eq!(store.load(&copy).unwrap().0.name, "Photo cleanup (copy)");
        assert_eq!(store.list().0.len(), 2);
    }

    /// The store is handed paths by a UI list. One that is not ours is not ours
    /// to delete.
    #[test]
    fn a_path_outside_the_store_is_refused() {
        let dir = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let outsider = elsewhere.path().join("mine.toml");
        std::fs::write(&outsider, "[[step]]\nop = \"space_trim\"\n").unwrap();

        let store = PresetStore::new(dir.path());
        assert!(matches!(
            store.delete(&outsider),
            Err(PresetError::Outside(_))
        ));
        assert!(matches!(
            store.rename(&outsider, "x"),
            Err(PresetError::Outside(_))
        ));
        assert!(outsider.exists(), "and it is still there");
    }

    #[test]
    fn export_then_import_round_trips_through_a_file_outside_the_store() {
        let dir = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());

        let shared = elsewhere.path().join("shared.toml");
        store.export(&sample(), &shared).unwrap();

        let (path, notes) = store.import(&shared).unwrap();
        assert!(notes.is_empty());
        assert_eq!(store.load(&path).unwrap().0, sample());
    }

    #[test]
    fn a_preset_needs_a_name_to_be_saved() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        assert!(matches!(
            store.save(&Preset::default()),
            Err(PresetError::EmptyName)
        ));
    }

    /// An empty preset is legal — the **New** button makes one.
    #[test]
    fn an_empty_preset_saves_and_loads() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        let path = store.save(&Preset::new("Empty")).unwrap();
        let (back, _) = store.load(&path).unwrap();
        assert!(back.is_empty());
        assert_eq!(back.summary(), "no operations");
    }

    #[test]
    fn a_display_name_becomes_a_file_name_that_works_on_windows_too() {
        for (name, expected) in [
            ("Photo cleanup", "Photo cleanup"),
            ("a/b", "a_b"),
            (r"C:\temp", "C_temp"),
            ("what? *now*", "what_ _now_"),
            ("trailing.  ", "trailing"),
            ("CON", "CON_"),
            ("lpt1", "lpt1_"),
            ("", "preset"),
            // Not "preset": the display name inside the file is still "///",
            // and inventing a name here could collide with a real one.
            ("///", "_"),
        ] {
            let stem = file_stem_for(name);
            assert_eq!(stem, expected, "{name:?}");
            naming::WINDOWS
                .validate_component(&format!("{stem}.toml"))
                .unwrap_or_else(|e| panic!("{name:?} produced an illegal file name: {e}"));
        }
    }

    #[test]
    fn a_very_long_name_is_trimmed_to_something_storable() {
        let stem = file_stem_for(&"ünïcödé ".repeat(40));
        assert!(stem.encode_utf16().count() <= 96);
        naming::WINDOWS
            .validate_component(&format!("{stem} (999).toml"))
            .expect("must still fit with a collision suffix");
    }

    /// An interrupted save must not leave a half-written preset — or a stray
    /// temporary file next to it.
    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let dir = TempDir::new().unwrap();
        let store = PresetStore::new(dir.path());
        store.save(&sample()).unwrap();
        store.save(&sample()).unwrap();

        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("tmp"))
            .collect();
        assert!(strays.is_empty(), "{strays:?}");
    }

    #[test]
    fn a_preset_runs_the_pipeline_it_stored() {
        let preset = sample();
        let entry = crate::model::FileEntry::synthetic("/music/my_song.mp3");
        let out = crate::evaluate_all(std::slice::from_ref(&entry), &preset.pipeline());
        assert_eq!(out[0].as_ref().unwrap().name, "My Song.mp3");
    }

    /// A preset must not carry the session's random seed.
    ///
    /// The GUI reseeds once per session, so whatever was live when the user
    /// pressed *Save as preset* would otherwise be written into the file — and
    /// every later run of that preset would produce the same "random" names,
    /// forever. Zero is the "nobody has chosen one" sentinel (P16); pinning a
    /// seed on purpose is a job file's job.
    #[test]
    fn a_saved_preset_never_carries_a_seed() {
        let mut preset = sample();
        preset.settings.reseed();
        assert_ne!(preset.settings.seed, 0, "the fixture must have one to drop");

        let document = preset.to_document();
        assert_eq!(document.settings.seed, 0);

        // And the same through the text, which is what actually lands on disk.
        let text = document.to_toml().expect("writable");
        assert!(!text.contains("seed"), "{text}");
    }
}

/// The six a fresh install starts with, compiled into the binary as our own
/// data (D6).
///
/// Embedded rather than installed beside the executable, for the two reasons
/// the scripts are: a portable build stays one file, and the defaults are our
/// own data.
pub const DEFAULTS: &[(&str, &str)] = &[
    (
        "Basic filename cleanup",
        include_str!("../data/presets/Basic filename cleanup.toml"),
    ),
    (
        "Add prefix to filename",
        include_str!("../data/presets/Add prefix to filename.toml"),
    ),
    (
        "Add suffix to end of filename",
        include_str!("../data/presets/Add suffix to end of filename.toml"),
    ),
    (
        "Create numbered sequence",
        include_str!("../data/presets/Create numbered sequence.toml"),
    ),
    (
        "Rename Mp3s as Artist - Title",
        include_str!("../data/presets/Rename Mp3s as Artist - Title.toml"),
    ),
    (
        "Sync file date with image Exif date",
        include_str!("../data/presets/Sync file date with image Exif date.toml"),
    ),
];

#[cfg(test)]
mod default_tests {
    use super::*;

    /// > *"Six default presets ship with the app."*
    ///
    /// And every one of them has to actually load, which is the half a count
    /// would not catch: these are hand-written TOML compiled into the binary,
    /// so a typo in one is a preset nobody can use and nothing else notices.
    #[test]
    fn the_six_that_ship_all_load_and_have_something_to_do() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PresetStore::new(dir.path().join("presets"));
        assert_eq!(store.seed_defaults().unwrap(), 6);

        let (entries, problems) = store.list();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(entries.len(), 6);
        for entry in &entries {
            assert!(!entry.name.trim().is_empty(), "{entry:?}");
            assert!(!entry.description.trim().is_empty(), "{entry:?}");
            assert!(entry.steps > 0, "{} does nothing", entry.name);
        }
    }

    /// A deleted preset is a deleted **menu item** once the Explorer menu is
    /// installed, so bringing it back on the next start is the app arguing with
    /// the user about their own right-click menu. Seeding once, into a folder
    /// that does not exist, cannot do that — which is why this differs from the
    /// script store, where re-seeding a missing file is harmless.
    #[test]
    fn a_preset_the_user_deleted_stays_deleted() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PresetStore::new(dir.path().join("presets"));
        assert_eq!(store.seed_defaults().unwrap(), 6);

        let (entries, _) = store.list();
        let doomed = entries.first().expect("six of them").path.clone();
        std::fs::remove_file(&doomed).unwrap();

        assert_eq!(store.seed_defaults().unwrap(), 0, "the folder is there now");
        assert_eq!(store.list().0.len(), 5, "and it stays gone");
    }

    /// The names are what the Explorer menu will show, so two of them sharing
    /// one would give two identical menu items — the `Ambiguous` case, shipped
    /// by us rather than made by the user.
    #[test]
    fn no_two_of_the_defaults_share_a_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PresetStore::new(dir.path().join("presets"));
        store.seed_defaults().unwrap();

        let mut names: Vec<String> = store.list().0.into_iter().map(|e| e.name).collect();
        names.sort();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "{names:?}");
    }
}
