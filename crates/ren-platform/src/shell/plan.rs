//! The Explorer menu, as data.
//!
//! Right-clicking one or more files in Windows Explorer offers a RenameIt
//! menu, from which the selection can be renamed using one of your presets.
//!
//! No `cfg(windows)` anywhere in this file, deliberately. The registry layout
//! is the half that can be silently wrong — a `(Default)` where there must not
//! be one, a sort prefix one digit narrower than its neighbour's, an ampersand
//! that turns into a keyboard accelerator — and it is exactly the half a Linux
//! CI runner can check. `apply` is a loop over two enum arms.
//!
//! # Why a cascade, and why these keys
//!
//! A preset menu could be built on an in-process COM handler, but the GPL-3.0
//! options are barred outright by **D2** — and such a handler's config is
//! seventeen lines in which every item is a plain command line, so there is no
//! COM *behaviour* to
//! reproduce, only a command line and a place to hang it.
//!
//! `ExtendedSubCommandsKey` is that place. Microsoft: *"you can register any
//! custom verbs under the `HKEY_CURRENT_USER\Software\Classes` subkey. The main
//! advantage of doing so is that elevated permission is not required."* The
//! alternative, `SubCommands`, resolves through a `CommandStore` that the same
//! page says needs HKLM.

use std::path::Path;

/// One preset, as the menu needs it.
///
/// **Not** `ren_core::PresetEntry`, and not because of taste: `ren-core`
/// already depends on this crate, so taking that type would be a cycle. It is
/// the honest shape anyway — a menu item is a caption and a thing to run, and
/// pinning that down to two fields says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuPreset<'a> {
    /// What the menu item says.
    pub name: &'a str,
    /// What the command runs. **A path, never the name**: two presets may share
    /// a display name, which is why `PresetStore::load_named` returns
    /// `Ambiguous` rather than guessing. A menu built on names would ship that
    /// ambiguity to every right-click.
    pub file: &'a Path,
}

/// A registry value, in the two flavours this menu uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// `REG_SZ`. Never `REG_EXPAND_SZ`: a preset called `50% off` has to reach
    /// the menu with its percent sign rather than an expanded environment
    /// variable.
    Sz(String),
    /// `REG_DWORD`. Two uses: `CommandFlags`, which draws the line between the
    /// fixed items and the presets, and the version marker on each verb root.
    Dword(u32),
}

/// One registry operation, all of them under `HKEY_CURRENT_USER`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Delete a key and everything beneath it. Already-absent is success.
    DeleteTree { key: String },
    /// Create the key if needed and set one value. An empty `name` would be
    /// the key's `(Default)`, which this plan never emits — see
    /// [`ShellPlan::build`].
    Set {
        key: String,
        name: String,
        value: Value,
    },
}

/// The verb key name under each class, and the only key name we own.
pub const VERB: &str = "RenameIt";

/// A `DWORD` on each verb root saying which layout wrote it.
///
/// Without it, "is the menu installed?" is answered by a key that a **D132**
/// install also has — the single *"Open with &RenameIt"* verb lived at exactly
/// `…\shell\RenameIt`. Settings would report a working cascade to a user who
/// has a plain item, and nothing would ever repair it.
///
/// Bump it when a stale install would get the new layout wrong. A name nobody
/// else uses, because the shell's own vocabulary for a verb key (`MUIVerb`,
/// `Icon`, `CommandFlags`, `Position`, `Extended`, `NeverDefault`…) is open to
/// additions and `Version` is the kind of word that gets added.
pub const VERSION_VALUE: &str = "RenameItMenuVersion";
pub const MENU_VERSION: u32 = 1;

/// The submenu's own label.
pub const MENU_LABEL: &str = "RenameIt";

/// Where the shared child menus live, **relative to HKCR** — which is exactly
/// the form `ExtendedSubCommandsKey` wants written into it.
///
/// Two of them, because a folder's *background* has no selection: *Start and
/// load selected files* and *Copy filenames to clipboard* would be items with
/// nothing to act on.
pub const ITEMS_CONTAINER: &str = "RenameIt.ContextMenu";
pub const BACKGROUND_CONTAINER: &str = "RenameIt.ContextMenu.Background";

const CLASSES_ROOT: &str = r"Software\Classes";

/// What Explorer substitutes, and whether we quote it.
///
/// **Bare** in the item container. Under `MultiSelectModel = Player` the shell
/// substitutes the whole selection, already quoted per item, so `"%1"` there
/// would produce `""a" "b""`. **Quoted** in the background container, where
/// `%V` is always exactly one folder.
///
/// Microsoft documents neither of those precisely, which is why both are named
/// constants with a row in `docs/manual-checks.md` rather than literals buried
/// in a format string.
const SELECTION_TOKEN: &str = "%1";
const BACKGROUND_TOKEN: &str = "\"%V\"";

/// At most this many presets reach the menu.
///
/// Not a registry limit — a human one. A forty-item cascade is already past
/// useful, and a cascade nobody can scan is a cascade nobody uses.
pub const MAX_PRESETS: usize = 40;

/// `ECF_SEPARATORAFTER`.
///
/// **After**, not before: separator-*before* is honoured only at the top level
/// of a menu, so the line between the fixed items and the presets has to hang
/// off the *last fixed item* rather than the first preset.
const ECF_SEPARATOR_AFTER: u32 = 0x40;

/// A class the menu appears on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Class {
    /// The path under `Software\Classes`, `shell` included.
    pub shell_path: &'static str,
    /// Which container this class's cascade points at.
    pub container: &'static str,
    /// Whether a selection can exist here at all.
    pub has_selection: bool,
}

/// The three places the menu appears.
///
/// **`AllFilesystemObjects` rather than `*` and `Directory` separately.**
/// Explorer offers only the verbs common to *every* selected item, so with
/// files and folders registered under different classes, selecting one of each
/// and right-clicking makes the whole menu disappear. One pseudo-class covers
/// both, and mixed selections are the normal case in a folder of photographs
/// and their subfolders.
pub const CLASSES: [Class; 3] = [
    Class {
        shell_path: r"AllFilesystemObjects\shell",
        container: ITEMS_CONTAINER,
        has_selection: true,
    },
    Class {
        shell_path: r"Drive\shell",
        container: ITEMS_CONTAINER,
        has_selection: true,
    },
    Class {
        shell_path: r"Directory\Background\shell",
        container: BACKGROUND_CONTAINER,
        has_selection: false,
    },
];

/// The whole Explorer menu, as an ordered list of registry operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPlan {
    /// In order. Every [`Step::DeleteTree`] precedes every [`Step::Set`].
    pub steps: Vec<Step>,
}

impl ShellPlan {
    /// Every key this design owns, in delete order.
    ///
    /// The one function that stops register and unregister drifting apart:
    /// [`Self::build`] *starts* with these as deletes and [`Self::uninstall`]
    /// is nothing but these as deletes, so a fourth class or a third container
    /// reaches both callers or neither.
    pub fn owned_keys() -> Vec<String> {
        let mut keys: Vec<String> = CLASSES
            .iter()
            .map(|class| format!(r"{CLASSES_ROOT}\{}\{VERB}", class.shell_path))
            .collect();
        keys.push(format!(r"{CLASSES_ROOT}\{ITEMS_CONTAINER}"));
        keys.push(format!(r"{CLASSES_ROOT}\{BACKGROUND_CONTAINER}"));
        keys
    }

    /// The keys whose existence means "the menu is installed".
    pub fn verb_roots() -> Vec<String> {
        CLASSES
            .iter()
            .map(|class| format!(r"{CLASSES_ROOT}\{}\{VERB}", class.shell_path))
            .collect()
    }

    /// The menu with nothing in it: what removing it applies.
    pub fn uninstall() -> Self {
        Self {
            steps: Self::owned_keys()
                .into_iter()
                .map(|key| Step::DeleteTree { key })
                .collect(),
        }
    }

    /// The whole layout, from an executable and a list of presets.
    ///
    /// **It opens with deletes, and that is the upgrade path rather than
    /// tidiness.** A D132-era install has a `(Default)` and a `\command` under
    /// `…\shell\RenameIt`; a cascade parent carrying either renders as a plain,
    /// non-cascading item, so the old shape has to go before the new one is
    /// written.
    ///
    /// **No `(Default)` is emitted anywhere.** Microsoft says three times that
    /// a cascade parent's default value must be unset, with `MUIVerb` carrying
    /// the text instead. Stating it as a rule for the whole plan rather than a
    /// special case for the parent is what makes it impossible to half-apply.
    pub fn build(exe: &Path, presets: &[MenuPreset<'_>]) -> Self {
        let exe = exe.display().to_string();
        let icon = format!("\"{exe}\",0");
        let mut steps = Self::uninstall().steps;

        let presets = &presets[..presets.len().min(MAX_PRESETS)];

        for container in [ITEMS_CONTAINER, BACKGROUND_CONTAINER] {
            let selection = container == ITEMS_CONTAINER;
            let token = if selection {
                SELECTION_TOKEN
            } else {
                BACKGROUND_TOKEN
            };
            let root = format!(r"{CLASSES_ROOT}\{container}\shell");

            // The fixed items. Two of the three have nothing to act on without
            // a selection, so the background menu carries only the first.
            let mut fixed: Vec<(&str, &str, String)> = vec![(
                "010_start-here",
                "Start from this folder",
                format!("\"{exe}\" --from-shell --start-in {token}"),
            )];
            if selection {
                fixed.push((
                    "020_load-selection",
                    "Start and load selected files",
                    format!("\"{exe}\" --from-shell {token}"),
                ));
                fixed.push((
                    "030_copy-names",
                    "Copy filenames to clipboard",
                    format!("\"{exe}\" --from-shell --copy-names {token}"),
                ));
            }

            let last_fixed = fixed.len() - 1;
            for (index, (key, label, command)) in fixed.iter().enumerate() {
                // The separator hangs off the last fixed item, and only when
                // there is something below it — otherwise a fresh install with
                // no presets draws a line under its last item with nothing
                // after it.
                let separator = index == last_fixed && !presets.is_empty();
                Self::push_item(
                    &mut steps,
                    &format!(r"{root}\{key}"),
                    label,
                    &icon,
                    command,
                    selection,
                    separator.then_some(ECF_SEPARATOR_AFTER),
                );
            }

            for (index, preset) in presets.iter().enumerate() {
                let key = format!(r"{root}\{}", preset_key(index, preset.name));
                let command = format!(
                    "\"{exe}\" --from-shell --preset \"{}\" {token}",
                    preset.file.display()
                );
                Self::push_item(
                    &mut steps,
                    &key,
                    &menu_label(preset.name),
                    &icon,
                    &command,
                    selection,
                    None,
                );
            }
        }

        // **The verb roots last.** They are what makes the menu appear, so
        // writing one before its container is a window in which a menu opened
        // mid-apply shows an empty cascade.
        for class in CLASSES {
            let key = format!(r"{CLASSES_ROOT}\{}\{VERB}", class.shell_path);
            steps.push(Step::sz(&key, "MUIVerb", MENU_LABEL));
            steps.push(Step::sz(&key, "Icon", &icon));
            steps.push(Step::sz(&key, "ExtendedSubCommandsKey", class.container));
            if class.has_selection {
                steps.push(Step::sz(&key, "MultiSelectModel", "Player"));
            }
            // Insurance: the shell falls back to the first `shell` subkey when
            // a class has no default verb, and ours becoming what a
            // double-click does would be a memorable bug.
            steps.push(Step::sz(&key, "NeverDefault", ""));
            // Last on the last key: whatever else fails, the marker that says
            // "this layout is complete" is the thing that did not get written.
            steps.push(Step::Set {
                key,
                name: VERSION_VALUE.to_owned(),
                value: Value::Dword(MENU_VERSION),
            });
        }

        Self { steps }
    }

    /// One menu item: its values, then the `\command` subkey underneath it.
    ///
    /// **Every value on the item comes before the subkey**, which is not
    /// cosmetic once `to_reg` exists — a value written after its own child
    /// re-opens a `[section]` further down the file, and a human comparing the
    /// output against Microsoft's sample has to notice that the second
    /// `030_copy-names` block is the same key again. The separator flag was
    /// exactly that, which is why it arrives here as a parameter rather than an
    /// afterthought at the call site.
    fn push_item(
        steps: &mut Vec<Step>,
        key: &str,
        label: &str,
        icon: &str,
        command: &str,
        selection: bool,
        flags: Option<u32>,
    ) {
        steps.push(Step::sz(key, "MUIVerb", label));
        steps.push(Step::sz(key, "Icon", icon));
        if selection {
            steps.push(Step::sz(key, "MultiSelectModel", "Player"));
        }
        if let Some(flags) = flags {
            steps.push(Step::Set {
                key: key.to_owned(),
                name: "CommandFlags".to_owned(),
                value: Value::Dword(flags),
            });
        }
        steps.push(Step::sz(&format!(r"{key}\command"), "", command));
    }

    /// The menu's labels, in menu order — for Settings to draw what will be
    /// written rather than a description of it.
    pub fn menu_labels(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter_map(|step| match step {
                Step::Set { key, name, value } if name == "MUIVerb" && key.contains(r"\shell\") => {
                    match value {
                        Value::Sz(text) => Some(text.as_str()),
                        Value::Dword(_) => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// The plan as `.reg` text — for the snapshot test, and for a bug report a
    /// user can paste into a reply.
    ///
    /// It has to be **importable**, not merely readable: half the point of the
    /// snapshot is that a human can hold it beside Microsoft's own sample, and
    /// the other half is that a user on a broken install can double-click it.
    /// Every value here contains quotes — `Icon` is `"<exe>",0` and every
    /// command starts with a quoted path — so unescaped output would have
    /// looked fine and imported as garbage.
    pub fn to_reg(&self) -> String {
        let mut out = String::from("Windows Registry Editor Version 5.00\n");
        let mut current = String::new();
        for step in &self.steps {
            match step {
                Step::DeleteTree { key } => {
                    current.clear();
                    out.push_str(&format!("\n[-HKEY_CURRENT_USER\\{key}]\n"));
                }
                Step::Set { key, name, value } => {
                    if *key != current {
                        out.push_str(&format!("\n[HKEY_CURRENT_USER\\{key}]\n"));
                        current = key.clone();
                    }
                    let name = if name.is_empty() {
                        "@".to_owned()
                    } else {
                        format!("\"{name}\"")
                    };
                    match value {
                        Value::Sz(text) => {
                            // Backslashes first, or the escape added for a
                            // quote gets escaped in turn.
                            let text = text.replace('\\', r"\\").replace('"', "\\\"");
                            out.push_str(&format!("{name}=\"{text}\"\n"));
                        }
                        Value::Dword(n) => out.push_str(&format!("{name}=dword:{n:08x}\n")),
                    }
                }
            }
        }
        out
    }
}

impl Step {
    fn sz(key: &str, name: &str, value: &str) -> Self {
        Self::Set {
            key: key.to_owned(),
            name: name.to_owned(),
            value: Value::Sz(value.to_owned()),
        }
    }
}

/// A registry key name for a preset: a sort prefix and a sanitised name.
///
/// The prefix is **three digits from 100**, so `100 < 101 < … < 109 < 110`
/// under the alphabetical sort a cascade uses — two digits break between the
/// ninth and tenth preset, which is the classic version of this bug and looks
/// fine in every test with fewer than ten.
///
/// `\` is the one character that must go: it would open a subkey level, putting
/// the item in a phantom sub-submenu or writing outside the container we clean
/// up. `&`, `%`, spaces and non-ASCII are all legal in a key name, and the key
/// is never shown to anyone.
fn preset_key(index: usize, name: &str) -> String {
    let mut slug: String = name
        .chars()
        .map(|c| if c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    slug = slug.trim().trim_matches('.').to_owned();
    if slug.chars().count() > 48 {
        slug = slug.chars().take(48).collect();
    }
    if slug.is_empty() {
        slug = "preset".to_owned();
    }
    format!("{:03}_{slug}", 100 + index)
}

/// A preset's display name as a menu label.
///
/// `&` is a **mnemonic** — the shipped verb text is deliberately `"Open with
/// &RenameIt"`, so the shell really does process it, and a preset called
/// `Rock & Roll` would otherwise read as "Rock  Roll" with an R accelerator.
///
/// A leading `@` is neutralised because `MUIVerb` accepts `@file,resource` as
/// a resource reference; a name that began with one would render empty, which
/// is harder to diagnose than rendering wrong.
fn menu_label(name: &str) -> String {
    let escaped = name.replace('&', "&&");
    if escaped.starts_with('@') {
        format!(" {escaped}")
    } else {
        escaped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\RenameIt\renameit.exe")
    }

    fn preset_files(names: &[&str]) -> Vec<(String, PathBuf)> {
        names
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    PathBuf::from(format!(r"C:\presets\{name}.toml")),
                )
            })
            .collect()
    }

    fn menu(files: &[(String, PathBuf)]) -> ShellPlan {
        let presets: Vec<MenuPreset<'_>> = files
            .iter()
            .map(|(name, file)| MenuPreset { name, file })
            .collect();
        ShellPlan::build(&exe(), &presets)
    }

    /// The one input the snapshot is taken from.
    ///
    /// Two presets, chosen so the pair carries every escaping rule at once: an
    /// ampersand that must double in the label and not in the key, a percent
    /// sign that must survive because nothing here is `REG_EXPAND_SZ`, spaces
    /// in both the program path and a preset path, and an order that only
    /// holds because the key prefix carries it (`Rock` would sort after `50`).
    fn snapshot_plan() -> ShellPlan {
        let presets = [
            MenuPreset {
                name: "Rock & Roll",
                file: Path::new(r"C:\Users\mk\presets\Rock & Roll.toml"),
            },
            MenuPreset {
                name: "50% off",
                file: Path::new(r"C:\Users\mk\presets\50% off.toml"),
            },
        ];
        ShellPlan::build(Path::new(r"C:\Apps\RenameIt\renameit.exe"), &presets)
    }

    /// Is this key inside that container?
    ///
    /// **Not `contains(container)`** — `RenameIt.ContextMenu` is a *prefix* of
    /// `RenameIt.ContextMenu.Background`, so the loose form silently counts the
    /// background store as part of the item store and doubles every total. It
    /// caught me on the first run of `the_menu_is_capped`, which read 80.
    /// Harmless in the registry itself (`.Background` is a sibling key, not a
    /// child, so deleting one does not touch the other) but not in a test.
    fn inside(key: &str, container: &str) -> bool {
        key.contains(&format!(r"\{container}\"))
    }

    fn sets(plan: &ShellPlan) -> Vec<(&str, &str, &Value)> {
        plan.steps
            .iter()
            .filter_map(|step| match step {
                Step::Set { key, name, value } => Some((key.as_str(), name.as_str(), value)),
                Step::DeleteTree { .. } => None,
            })
            .collect()
    }

    /// Microsoft says three times that a cascade parent's `(Default)` must be
    /// unset. Stated as a rule for the **whole plan** rather than a special
    /// case for the parent, so it cannot be half-applied — and because the
    /// shape it replaces, D132's `set_string(&base, "", label)`, is exactly
    /// what a copy-paste would reintroduce.
    #[test]
    fn the_plan_never_sets_a_default_value_except_on_a_command() {
        let plan = menu(&preset_files(&["Tidy up"]));
        for (key, name, _) in sets(&plan) {
            if name.is_empty() {
                assert!(
                    key.ends_with(r"\command"),
                    "{key} sets (Default), which collapses a cascade into a plain item"
                );
            }
        }
    }

    /// Delete-then-write, in that order. Appending the deletes would erase
    /// everything the same plan had just written.
    #[test]
    fn every_delete_comes_before_every_write() {
        let plan = menu(&preset_files(&["A", "B"]));
        let last_delete = plan
            .steps
            .iter()
            .rposition(|s| matches!(s, Step::DeleteTree { .. }))
            .expect("the plan opens with deletes");
        let first_write = plan
            .steps
            .iter()
            .position(|s| matches!(s, Step::Set { .. }))
            .expect("and then writes");
        assert!(last_delete < first_write);
    }

    /// Stale keys can only be removed by deleting a subtree, because `reg` has
    /// no enumerate — so anything written outside one of the owned roots would
    /// be litter no uninstall could reach.
    #[test]
    fn everything_written_lives_under_something_deleted() {
        let plan = menu(&preset_files(&["A"]));
        let owned = ShellPlan::owned_keys();
        for (key, _, _) in sets(&plan) {
            assert!(
                owned
                    .iter()
                    .any(|root| key == root || key.starts_with(&format!("{root}\\"))),
                "{key} is written but never deleted"
            );
        }
    }

    /// The property that keeps register and unregister from drifting: both
    /// derive their deletes from one list.
    #[test]
    fn uninstall_deletes_exactly_what_build_creates() {
        let plan = menu(&preset_files(&["A", "B", "C"]));
        let built: Vec<&Step> = plan
            .steps
            .iter()
            .filter(|s| matches!(s, Step::DeleteTree { .. }))
            .collect();
        let uninstall = ShellPlan::uninstall();
        let removed: Vec<&Step> = uninstall.steps.iter().collect();
        assert_eq!(built, removed);
    }

    /// The verb root is what makes the menu appear, so writing one before its
    /// container is a window in which a menu opened mid-apply shows an empty
    /// cascade.
    #[test]
    fn the_verb_roots_are_written_last() {
        let plan = menu(&preset_files(&["A"]));
        let first_root = plan
            .steps
            .iter()
            .position(|s| matches!(s, Step::Set { key, .. } if key.ends_with(r"\shell\RenameIt")))
            .expect("four of them");
        let last_container = plan
            .steps
            .iter()
            .rposition(
                |s| matches!(s, Step::Set { key, .. } if key.contains("RenameIt.ContextMenu")),
            )
            .expect("and the items they point at");
        assert!(last_container < first_root);
    }

    /// A fresh install has presets, but a user who deleted them all still gets
    /// a working menu — and no trailing separator under its last item.
    #[test]
    fn an_empty_preset_list_still_gives_a_usable_menu() {
        let plan = menu(&[]);
        let labels = plan.menu_labels();
        assert!(labels.contains(&"Start from this folder"));
        assert!(labels.contains(&"Copy filenames to clipboard"));
        assert!(
            !sets(&plan)
                .iter()
                .any(|(_, name, _)| *name == "CommandFlags"),
            "a separator with nothing under it"
        );
    }

    /// A cascade sorts **alphabetically by key name**, so the order the plan
    /// lists items in is a suggestion unless the key carries it.
    ///
    /// Fixed items and presets in **one** sequence, which is where the real
    /// digit-boundary hazard is: the fixed prefixes and the preset prefixes
    /// have to be the same width. `010/020/030` against `100+` sorts; the
    /// tempting `10/20/30` against `100+` gives `10 < 100 < 20`, and the whole
    /// menu interleaves.
    ///
    /// The preset names are chosen so that a missing prefix cannot pass:
    /// `Preset 10` sorts between `Preset 1` and `Preset 2`.
    #[test]
    fn the_cascade_sorts_the_items_into_the_order_the_plan_listed_them() {
        let names: Vec<String> = (0..12).map(|i| format!("Preset {i}")).collect();
        let files = preset_files(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let plan = menu(&files);

        let mut keys: Vec<&str> = sets(&plan)
            .iter()
            .filter(|(key, name, _)| *name == "MUIVerb" && inside(key, ITEMS_CONTAINER))
            .map(|(key, _, _)| *key)
            .collect();
        assert_eq!(keys.len(), 15, "three fixed items and twelve presets");

        let given = keys.clone();
        keys.sort();
        assert_eq!(keys, given, "the cascade's own sort must not reorder them");
    }

    /// Names are not unique, which is why the command carries the path. Two
    /// presets with one name must give two items rather than one.
    #[test]
    fn two_presets_with_one_name_get_two_items() {
        let files = vec![
            ("Tidy".to_owned(), PathBuf::from(r"C:\presets\a.toml")),
            ("Tidy".to_owned(), PathBuf::from(r"C:\presets\b.toml")),
        ];
        let plan = menu(&files);
        let commands: Vec<&str> = sets(&plan)
            .iter()
            .filter(|(key, _, _)| key.ends_with(r"\command"))
            .filter_map(|(_, _, v)| match v {
                Value::Sz(text) if text.contains("--preset") => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(commands.iter().any(|c| c.contains(r"a.toml")));
        assert!(commands.iter().any(|c| c.contains(r"b.toml")));
    }

    /// `&` is a mnemonic in a menu label and is not one in a key name, so
    /// doubling both is as wrong as doubling neither.
    #[test]
    fn an_ampersand_is_doubled_in_the_label_and_left_alone_in_the_key() {
        let files = preset_files(&["Rock & Roll"]);
        let plan = menu(&files);
        assert!(plan.menu_labels().contains(&"Rock && Roll"));
        assert!(
            sets(&plan)
                .iter()
                .any(|(key, _, _)| key.contains("100_Rock & Roll")),
            "the key name is not a menu label"
        );
    }

    /// `MUIVerb` reads a leading `@` as `@file,resource`, so a preset named
    /// `@work` would render as an empty item.
    #[test]
    fn a_label_never_begins_with_an_at_sign() {
        let files = preset_files(&["@work photos"]);
        let plan = menu(&files);
        assert!(
            plan.menu_labels().iter().all(|l| !l.starts_with('@')),
            "{:?}",
            plan.menu_labels()
        );
    }

    /// `REG_SZ`, never `REG_EXPAND_SZ` — or `50% off` becomes whatever `%
    /// off%` expands to, which is usually nothing.
    #[test]
    fn a_percent_sign_survives_because_every_label_is_a_plain_string() {
        let files = preset_files(&["50% off"]);
        let plan = menu(&files);
        assert!(plan.menu_labels().contains(&"50% off"));
        for (_, _, value) in sets(&plan) {
            assert!(
                matches!(value, Value::Sz(_) | Value::Dword(_)),
                "no expandable strings"
            );
        }
    }

    /// A backslash in a key name opens a subkey level, which would put the
    /// item in a phantom sub-submenu or write outside the container we delete.
    #[test]
    fn a_backslash_in_a_name_cannot_open_a_subkey() {
        let files = vec![(r"a\b".to_owned(), PathBuf::from(r"C:\presets\odd.toml"))];
        let plan = menu(&files);
        let root = format!(r"{CLASSES_ROOT}\{ITEMS_CONTAINER}\shell");
        for (key, _, _) in sets(&plan) {
            if let Some(rest) = key.strip_prefix(&format!("{root}\\")) {
                let depth = rest.trim_end_matches(r"\command").matches('\\').count();
                assert_eq!(depth, 0, "{key} is more than one level below the container");
            }
        }
    }

    /// A folder's background has no selection, so two of the three fixed items
    /// would be present and meaningless there — and `%1` means nothing.
    #[test]
    fn the_background_menu_never_mentions_a_selection() {
        let plan = menu(&preset_files(&["Tidy"]));
        for (key, _, value) in sets(&plan) {
            if !inside(key, BACKGROUND_CONTAINER) {
                continue;
            }
            if let Value::Sz(text) = value {
                assert!(!text.contains("--copy-names"), "{key}: {text}");
                assert!(!text.contains(SELECTION_TOKEN), "{key}: {text}");
            }
        }
        let labels: Vec<&str> = plan.menu_labels();
        assert_eq!(
            labels
                .iter()
                .filter(|l| **l == "Start and load selected files")
                .count(),
            1,
            "the item container only"
        );
    }

    /// The exe and the preset file are quoted; the selection token is not.
    ///
    /// Under `Player` the shell substitutes an already-quoted list, so `"%1"`
    /// would give `""a" "b""`. `%V` is one folder and must be quoted. Neither
    /// is documented precisely, which is why both have a manual-check row.
    #[test]
    fn the_command_quotes_the_paths_and_not_the_selection() {
        let files = preset_files(&["Tidy up"]);
        let plan = menu(&files);
        for (key, _, value) in sets(&plan) {
            if !key.ends_with(r"\command") {
                continue;
            }
            let Value::Sz(command) = value else { continue };
            assert!(
                command.starts_with("\"C:\\Program Files\\RenameIt\\renameit.exe\""),
                "{command}"
            );
            if inside(key, BACKGROUND_CONTAINER) {
                assert!(command.contains("\"%V\""), "{command}");
            } else {
                assert!(command.ends_with(" %1"), "{command}");
                assert!(!command.contains("\"%1\""), "{command}");
            }
            if command.contains("--preset") {
                assert!(
                    command.contains("\"C:\\presets\\Tidy up.toml\""),
                    "{command}"
                );
            }
        }
    }

    /// Windows caps a static verb's command line at 2000 characters, so the
    /// fixed part is a budget: everything it spends is a file the user cannot
    /// select.
    #[test]
    fn the_command_leaves_room_for_a_real_selection() {
        let files = preset_files(&["A reasonably descriptive preset name"]);
        let plan = menu(&files);
        let longest = sets(&plan)
            .iter()
            .filter(|(key, _, _)| key.ends_with(r"\command"))
            .filter_map(|(_, _, v)| match v {
                Value::Sz(text) => Some(text.len()),
                Value::Dword(_) => None,
            })
            .max()
            .expect("commands");
        assert!(longest < 400, "{longest} characters of fixed overhead");
    }

    /// A cascade of forty is already past useful.
    #[test]
    fn the_menu_is_capped() {
        let names: Vec<String> = (0..200).map(|i| format!("P{i}")).collect();
        let files = preset_files(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let plan = menu(&files);
        let items = sets(&plan)
            .iter()
            .filter(|(key, name, _)| {
                *name == "MUIVerb" && inside(key, ITEMS_CONTAINER) && key.contains(r"\shell\1")
            })
            .count();
        assert_eq!(items, MAX_PRESETS);
    }

    /// The whole menu, as a file a human can read and Windows can import.
    ///
    /// The only snapshot in this module, and it is here because every other
    /// test asserts one property in isolation — this is the one artifact that
    /// can be held beside Microsoft's own `ExtendedSubCommandsKey` sample and
    /// compared line for line. Four things are visible here and nowhere else:
    /// the deletes standing alone at the top; `Rock & Roll` sorting *before*
    /// `50% off`, because the key prefix and not the name carries the order;
    /// the separator flag on the last fixed item of each store; and the three
    /// verb roots arriving last.
    ///
    /// Flush-left on purpose — indenting a raw string would indent the file it
    /// claims to be.
    ///
    /// If this fails, read the diff before updating it. A changed snapshot is
    /// a changed menu.
    #[test]
    fn the_whole_menu_as_a_registry_file() {
        assert_eq!(
            snapshot_plan().to_reg(),
            r#"Windows Registry Editor Version 5.00

[-HKEY_CURRENT_USER\Software\Classes\AllFilesystemObjects\shell\RenameIt]

[-HKEY_CURRENT_USER\Software\Classes\Drive\shell\RenameIt]

[-HKEY_CURRENT_USER\Software\Classes\Directory\Background\shell\RenameIt]

[-HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu]

[-HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu.Background]

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\010_start-here]
"MUIVerb"="Start from this folder"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"MultiSelectModel"="Player"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\010_start-here\command]
@="\"C:\\Apps\\RenameIt\\renameit.exe\" --from-shell --start-in %1"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\020_load-selection]
"MUIVerb"="Start and load selected files"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"MultiSelectModel"="Player"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\020_load-selection\command]
@="\"C:\\Apps\\RenameIt\\renameit.exe\" --from-shell %1"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\030_copy-names]
"MUIVerb"="Copy filenames to clipboard"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"MultiSelectModel"="Player"
"CommandFlags"=dword:00000040

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\030_copy-names\command]
@="\"C:\\Apps\\RenameIt\\renameit.exe\" --from-shell --copy-names %1"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\100_Rock & Roll]
"MUIVerb"="Rock && Roll"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"MultiSelectModel"="Player"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\100_Rock & Roll\command]
@="\"C:\\Apps\\RenameIt\\renameit.exe\" --from-shell --preset \"C:\\Users\\mk\\presets\\Rock & Roll.toml\" %1"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\101_50% off]
"MUIVerb"="50% off"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"MultiSelectModel"="Player"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu\shell\101_50% off\command]
@="\"C:\\Apps\\RenameIt\\renameit.exe\" --from-shell --preset \"C:\\Users\\mk\\presets\\50% off.toml\" %1"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu.Background\shell\010_start-here]
"MUIVerb"="Start from this folder"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"CommandFlags"=dword:00000040

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu.Background\shell\010_start-here\command]
@="\"C:\\Apps\\RenameIt\\renameit.exe\" --from-shell --start-in \"%V\""

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu.Background\shell\100_Rock & Roll]
"MUIVerb"="Rock && Roll"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu.Background\shell\100_Rock & Roll\command]
@="\"C:\\Apps\\RenameIt\\renameit.exe\" --from-shell --preset \"C:\\Users\\mk\\presets\\Rock & Roll.toml\" \"%V\""

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu.Background\shell\101_50% off]
"MUIVerb"="50% off"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"

[HKEY_CURRENT_USER\Software\Classes\RenameIt.ContextMenu.Background\shell\101_50% off\command]
@="\"C:\\Apps\\RenameIt\\renameit.exe\" --from-shell --preset \"C:\\Users\\mk\\presets\\50% off.toml\" \"%V\""

[HKEY_CURRENT_USER\Software\Classes\AllFilesystemObjects\shell\RenameIt]
"MUIVerb"="RenameIt"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"ExtendedSubCommandsKey"="RenameIt.ContextMenu"
"MultiSelectModel"="Player"
"NeverDefault"=""
"RenameItMenuVersion"=dword:00000001

[HKEY_CURRENT_USER\Software\Classes\Drive\shell\RenameIt]
"MUIVerb"="RenameIt"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"ExtendedSubCommandsKey"="RenameIt.ContextMenu"
"MultiSelectModel"="Player"
"NeverDefault"=""
"RenameItMenuVersion"=dword:00000001

[HKEY_CURRENT_USER\Software\Classes\Directory\Background\shell\RenameIt]
"MUIVerb"="RenameIt"
"Icon"="\"C:\\Apps\\RenameIt\\renameit.exe\",0"
"ExtendedSubCommandsKey"="RenameIt.ContextMenu.Background"
"NeverDefault"=""
"RenameItMenuVersion"=dword:00000001
"#
        );
    }

    /// A **D132** install has a key at exactly `…\shell\RenameIt` too — it was
    /// the single *"Open with &RenameIt"* verb. Existence alone therefore
    /// cannot answer "is the menu installed?", and the marker that can has to
    /// be on every root, because a half-applied plan is the case it is for.
    #[test]
    fn every_verb_root_carries_the_version_marker() {
        let plan = menu(&preset_files(&["A"]));
        let marked = sets(&plan)
            .iter()
            .filter(|(key, name, value)| {
                key.ends_with(&format!(r"\{VERB}"))
                    && *name == VERSION_VALUE
                    && **value == Value::Dword(MENU_VERSION)
            })
            .count();
        assert_eq!(marked, CLASSES.len());
    }

    /// Same input, same plan — the real form of "stable", and what stops a
    /// `HashMap` iteration order sneaking into the builder.
    #[test]
    fn the_plan_is_deterministic() {
        let files = preset_files(&["A", "B", "C"]);
        assert_eq!(menu(&files), menu(&files));
    }

    /// Every path is under `HKCU\Software\Classes`. HKCU-only is the whole
    /// design — no administrator to add it or to remove it (D132) — and a test
    /// is cheaper than noticing in review.
    #[test]
    fn every_key_is_under_the_current_user() {
        let plan = menu(&preset_files(&["A"]));
        for step in &plan.steps {
            let key = match step {
                Step::DeleteTree { key } => key,
                Step::Set { key, .. } => key,
            };
            assert!(key.starts_with(CLASSES_ROOT), "{key}");
        }
    }
}
