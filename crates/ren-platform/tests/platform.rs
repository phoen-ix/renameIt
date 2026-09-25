//! Behaviour every platform must exhibit, plus the per-OS capability contract.
//!
//! The Windows-only halves are the reason CI runs a `windows-latest` job at all
//! (M0's platform work): attributes and created dates cannot be tested anywhere
//! else.

use std::time::{Duration, SystemTime};

use ren_platform::{AttributeChange, CaseSensitivity, PlatformError, TimeChange, host};
// Only the capability-contract tests need this, and those are Unix-only.
#[cfg(unix)]
use ren_platform::Capability;
use tempfile::TempDir;

fn fixture() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let file = dir.path().join("sample.txt");
    std::fs::write(&file, b"contents").expect("write");
    (dir, file)
}

#[test]
fn rename_moves_the_file_and_leaves_the_contents_alone() {
    let (dir, file) = fixture();
    let target = dir.path().join("renamed.txt");
    host().rename(&file, &target).expect("rename");

    assert!(!file.exists());
    assert_eq!(std::fs::read(&target).unwrap(), b"contents");
}

#[test]
fn rename_refuses_to_overwrite_an_existing_target() {
    let (dir, file) = fixture();
    let occupied = dir.path().join("occupied.txt");
    std::fs::write(&occupied, b"do not lose me").unwrap();

    let err = host()
        .rename(&file, &occupied)
        .expect_err("must not clobber");
    assert!(
        matches!(err, PlatformError::TargetExists { .. }),
        "expected TargetExists, got {err:?}"
    );
    // The whole point: neither file was harmed.
    assert_eq!(std::fs::read(&occupied).unwrap(), b"do not lose me");
    assert_eq!(std::fs::read(&file).unwrap(), b"contents");
}

#[test]
fn unicode_and_emoji_names_survive_a_rename() {
    let (dir, file) = fixture();
    let target = dir.path().join("Ünïcödé — 日本語 🎵.txt");
    host().rename(&file, &target).expect("rename");
    assert_eq!(std::fs::read(&target).unwrap(), b"contents");
}

#[test]
fn read_only_can_be_set_and_cleared_everywhere() {
    let (_dir, file) = fixture();
    let platform = host();

    platform
        .set_attributes(
            &file,
            AttributeChange {
                read_only: Some(true),
                ..Default::default()
            },
        )
        .expect("set read-only");
    assert!(platform.get_attributes(&file).unwrap().read_only);

    platform
        .set_attributes(
            &file,
            AttributeChange {
                read_only: Some(false),
                ..Default::default()
            },
        )
        .expect("clear read-only");
    assert!(!platform.get_attributes(&file).unwrap().read_only);
}

#[test]
fn modified_time_can_be_set_everywhere() {
    let (_dir, file) = fixture();
    let platform = host();
    let target = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);

    platform
        .set_times(
            &file,
            TimeChange {
                modified: Some(target),
                ..Default::default()
            },
        )
        .expect("set mtime");

    let got = platform.get_times(&file).unwrap().modified.unwrap();
    let delta = got
        .duration_since(target)
        .or_else(|e| Ok::<_, std::time::SystemTimeError>(e.duration()))
        .unwrap();
    assert!(delta < Duration::from_secs(1), "mtime drifted by {delta:?}");
}

#[test]
fn case_sensitivity_is_probed_not_guessed() {
    let (dir, _file) = fixture();
    let verdict = host().case_sensitivity(dir.path());
    assert_ne!(
        verdict,
        CaseSensitivity::Unknown,
        "a writable tempdir must be probeable"
    );
    // The probe must not litter.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_lowercase().contains("case-probe"))
        .collect();
    assert!(leftovers.is_empty(), "probe left {leftovers:?} behind");
}

#[test]
fn naming_rules_match_the_host_filesystem_family() {
    let (dir, _file) = fixture();
    let rules = host().naming_rules(dir.path());
    if cfg!(windows) {
        assert_eq!(rules.id, "windows");
        assert!(rules.case_insensitive);
    } else {
        assert_eq!(rules.id, "posix");
        assert!(!rules.case_insensitive);
    }
}

// --- Capability contract (P5) ------------------------------------------------

#[cfg(unix)]
#[test]
fn setting_the_created_date_fails_loudly_on_unix() {
    let (_dir, file) = fixture();
    let err = host()
        .set_times(
            &file,
            TimeChange {
                created: Some(SystemTime::UNIX_EPOCH),
                ..Default::default()
            },
        )
        .expect_err("Linux cannot write a birth time");
    assert!(
        matches!(
            err,
            PlatformError::CapabilityUnsupported {
                capability: Capability::CreatedTime,
                ..
            }
        ),
        "expected an explicit capability error, got {err:?}"
    );
}

#[cfg(unix)]
#[test]
fn dos_only_attributes_fail_loudly_on_unix() {
    let (_dir, file) = fixture();
    let platform = host();
    for (change, expected) in [
        (
            AttributeChange {
                hidden: Some(true),
                ..Default::default()
            },
            Capability::HiddenAttribute,
        ),
        (
            AttributeChange {
                system: Some(true),
                ..Default::default()
            },
            Capability::SystemAttribute,
        ),
        (
            AttributeChange {
                archive: Some(true),
                ..Default::default()
            },
            Capability::ArchiveAttribute,
        ),
    ] {
        let err = platform.set_attributes(&file, change).expect_err("no-op");
        match err {
            PlatformError::CapabilityUnsupported { capability, .. } => {
                assert_eq!(capability, expected)
            }
            other => panic!("expected CapabilityUnsupported, got {other:?}"),
        }
    }
}

/// `Permissions::set_readonly(false)` sets **all three** write bits, so a 0644
/// file used to come back 0666. Undo restores the previous attributes, which
/// means that bug handed out group and world write access on every undo of a
/// read-only change.
#[cfg(unix)]
#[test]
fn clearing_read_only_never_widens_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let platform = host();
    for before in [0o644u32, 0o600, 0o755, 0o664, 0o444] {
        let (_dir, file) = fixture();
        std::fs::set_permissions(&file, PermissionsExt::from_mode(before)).expect("chmod");

        for read_only in [true, false] {
            platform
                .set_attributes(
                    &file,
                    AttributeChange {
                        read_only: Some(read_only),
                        ..Default::default()
                    },
                )
                .expect("set read-only");
        }

        let after = std::fs::metadata(&file).expect("stat").permissions().mode() & 0o777;
        // Granting the *owner* write is what "clear read-only" means, so that
        // bit may legitimately appear. Group and other never may.
        assert_eq!(
            after & !before & 0o022,
            0,
            "{before:o} became {after:o} — group or other gained write access"
        );
        assert_eq!(
            after & 0o200,
            0o200,
            "{before:o} became {after:o} — read-only was not actually cleared"
        );
    }

    // The two modes that matter in practice restore exactly.
    for mode in [0o644u32, 0o755] {
        let (_dir, file) = fixture();
        std::fs::set_permissions(&file, PermissionsExt::from_mode(mode)).expect("chmod");
        for read_only in [true, false] {
            platform
                .set_attributes(
                    &file,
                    AttributeChange {
                        read_only: Some(read_only),
                        ..Default::default()
                    },
                )
                .expect("set read-only");
        }
        let after = std::fs::metadata(&file).expect("stat").permissions().mode() & 0o777;
        assert_eq!(after, mode, "{mode:o} did not survive a set-then-clear");
    }
}

/// The undo path reads a file's attributes and writes them back. On Linux
/// `hidden` is readable from a leading dot but not writable, so without
/// `required_capabilities_from` that round trip failed on every dotfile — and
/// took the whole undo with it.
#[cfg(unix)]
#[test]
fn setting_an_attribute_to_the_value_it_already_has_is_not_an_error() {
    let dir = TempDir::new().expect("tempdir");
    let dotfile = dir.path().join(".hidden.txt");
    std::fs::write(&dotfile, b"x").expect("write");
    let platform = host();

    let current = platform.get_attributes(&dotfile).expect("get");
    assert!(current.hidden, "a leading dot reads as hidden");

    // Writing back exactly what was read is what undo does.
    platform
        .set_attributes(
            &dotfile,
            AttributeChange {
                read_only: Some(current.read_only),
                hidden: Some(current.hidden),
                system: Some(current.system),
                archive: Some(current.archive),
            },
        )
        .expect("writing back an unchanged attribute set must succeed");

    // And on a plain file, clearing the three bits Unix always reports as false.
    let (_dir2, plain) = fixture();
    platform
        .set_attributes(
            &plain,
            AttributeChange {
                hidden: Some(false),
                system: Some(false),
                archive: Some(false),
                ..Default::default()
            },
        )
        .expect("clearing bits that are already clear must succeed");
}

/// The permissive reading above must not swallow a real request. Guards the
/// fix against being over-applied.
#[cfg(unix)]
#[test]
fn changing_an_attribute_this_platform_cannot_write_still_fails_loudly() {
    let dir = TempDir::new().expect("tempdir");
    let dotfile = dir.path().join(".hidden.txt");
    std::fs::write(&dotfile, b"x").expect("write");
    let platform = host();

    // Un-hiding a dotfile is a real change, and Unix cannot make it.
    let err = platform
        .set_attributes(
            &dotfile,
            AttributeChange {
                hidden: Some(false),
                ..Default::default()
            },
        )
        .expect_err("un-hiding a dotfile is not a no-op");
    assert!(matches!(
        err,
        PlatformError::CapabilityUnsupported {
            capability: Capability::HiddenAttribute,
            ..
        }
    ));
}

/// The capability table is hand-written, so it can drift from what the setters
/// really accept. Every claim gets tried against a real file.
#[test]
fn every_capability_the_platform_claims_it_can_do_it_actually_does() {
    use ren_platform::Capability;

    let platform = host();
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);

    for capability in Capability::ALL {
        let (_dir, file) = fixture();
        let attempt = match capability {
            Capability::CreatedTime => platform.set_times(
                &file,
                TimeChange {
                    created: Some(time),
                    ..Default::default()
                },
            ),
            Capability::AccessedTime => platform.set_times(
                &file,
                TimeChange {
                    accessed: Some(time),
                    ..Default::default()
                },
            ),
            Capability::ModifiedTime => platform.set_times(
                &file,
                TimeChange {
                    modified: Some(time),
                    ..Default::default()
                },
            ),
            // Each asks for a *change*: the fixture is a plain writable file, so
            // every one of these differs from what it currently is.
            Capability::ReadOnlyAttribute => platform.set_attributes(
                &file,
                AttributeChange {
                    read_only: Some(true),
                    ..Default::default()
                },
            ),
            Capability::HiddenAttribute => platform.set_attributes(
                &file,
                AttributeChange {
                    hidden: Some(true),
                    ..Default::default()
                },
            ),
            Capability::SystemAttribute => platform.set_attributes(
                &file,
                AttributeChange {
                    system: Some(true),
                    ..Default::default()
                },
            ),
            Capability::ArchiveAttribute => platform.set_attributes(
                &file,
                AttributeChange {
                    archive: Some(true),
                    ..Default::default()
                },
            ),
            // Spawning a file manager in CI is not something a test should do.
            Capability::RevealInFileManager => continue,
            // Covered by `shell::tests`, which round-trips the real keys under
            // HKCU. Doing it here as well would leave this fixture's throwaway
            // executable path in the registry if the assertion below failed.
            Capability::ShellContextMenu => continue,
        };

        let refused = matches!(attempt, Err(PlatformError::CapabilityUnsupported { .. }));
        assert_eq!(
            platform.supports(capability),
            !refused,
            "{capability} is advertised as {} but the setter said otherwise: {attempt:?}",
            platform.supports(capability),
        );

        // Leave the file deletable: Windows refuses to remove a read-only
        // file, so the temp directory would outlive the test.
        let _ = platform.set_attributes(
            &file,
            AttributeChange {
                read_only: Some(false),
                ..Default::default()
            },
        );
    }
}

#[test]
fn unsupported_names_the_first_thing_the_platform_cannot_do() {
    use ren_platform::Capability;

    let platform = host();
    // Created first, deliberately: a user who ticked all three should read the
    // documented limitation, not whichever field was checked first.
    let wanted = TimeChange {
        created: Some(SystemTime::UNIX_EPOCH),
        accessed: Some(SystemTime::UNIX_EPOCH),
        modified: Some(SystemTime::UNIX_EPOCH),
    }
    .required_capabilities();
    assert_eq!(wanted[0], Capability::CreatedTime);

    if cfg!(unix) {
        assert_eq!(platform.unsupported(&wanted), Some(Capability::CreatedTime));
    } else {
        assert_eq!(platform.unsupported(&wanted), None);
    }
}

#[cfg(windows)]
#[test]
fn all_four_dos_attributes_round_trip_on_windows() {
    let (_dir, file) = fixture();
    let platform = host();

    platform
        .set_attributes(
            &file,
            AttributeChange {
                read_only: Some(true),
                hidden: Some(true),
                system: Some(true),
                archive: Some(false),
            },
        )
        .expect("set attributes");

    let got = platform.get_attributes(&file).unwrap();
    assert!(
        got.read_only && got.hidden && got.system && !got.archive,
        "{got:?}"
    );

    // Clear again so the tempdir can be removed.
    platform
        .set_attributes(
            &file,
            AttributeChange {
                read_only: Some(false),
                hidden: Some(false),
                system: Some(false),
                archive: Some(false),
            },
        )
        .expect("clear attributes");
}

#[cfg(windows)]
#[test]
fn the_created_date_can_actually_be_set_on_windows() {
    let (_dir, file) = fixture();
    let platform = host();
    let target = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);

    platform
        .set_times(
            &file,
            TimeChange {
                created: Some(target),
                ..Default::default()
            },
        )
        .expect("set created");

    let got = platform.get_times(&file).unwrap().created.unwrap();
    let delta = got
        .duration_since(target)
        .or_else(|e| Ok::<_, std::time::SystemTimeError>(e.duration()))
        .unwrap();
    assert!(
        delta < Duration::from_secs(1),
        "created drifted by {delta:?}"
    );
}

#[cfg(windows)]
#[test]
fn a_case_only_rename_succeeds_on_a_case_insensitive_volume() {
    let (dir, file) = fixture();
    let target = dir.path().join("SAMPLE.TXT");
    host().rename(&file, &target).expect("case-only rename");

    let listed: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(listed, vec!["SAMPLE.TXT".to_string()]);
}

// --- No clobber, whatever is in the way (P13) --------------------------------

/// `symlink_metadata`, not `exists`, is what sees a link whose target is gone
/// — and `std::fs::rename` would replace the link without a word.
#[cfg(unix)]
#[test]
fn rename_refuses_to_overwrite_a_dangling_link() {
    let (dir, file) = fixture();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("gone"), &link).unwrap();

    let err = host().rename(&file, &link).expect_err("must not clobber");
    assert!(matches!(err, PlatformError::TargetExists { .. }), "{err:?}");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(&file).unwrap(), b"contents");
}

/// `rename(2)` replaces an empty directory with a directory.
#[cfg(unix)]
#[test]
fn rename_refuses_to_overwrite_an_empty_folder() {
    let dir = TempDir::new().unwrap();
    let from = dir.path().join("from");
    let to = dir.path().join("to");
    std::fs::create_dir(&from).unwrap();
    std::fs::write(from.join("inside.txt"), b"x").unwrap();
    std::fs::create_dir(&to).unwrap();

    let err = host().rename(&from, &to).expect_err("must not clobber");
    assert!(matches!(err, PlatformError::TargetExists { .. }), "{err:?}");
    assert!(from.join("inside.txt").exists());
    assert!(to.is_dir());
}

/// Two hard links to one file are the same inode, which the case-only
/// exemption used to accept — and `rename(2)` between two links of one file
/// is specified to do nothing and succeed. The run journalled a rename that
/// never happened.
#[cfg(unix)]
#[test]
fn rename_onto_a_hard_link_of_the_same_file_is_refused() {
    let (dir, file) = fixture();
    let other = dir.path().join("other.txt");
    std::fs::hard_link(&file, &other).unwrap();

    let err = host()
        .rename(&file, &other)
        .expect_err("a second name is not a free one");
    assert!(matches!(err, PlatformError::TargetExists { .. }), "{err:?}");
    assert!(file.exists() && other.exists());
}

/// The same, where the two links differ only in case — the shape a
/// case-only rename has. On a case-sensitive volume they are two names, and
/// the rename must be refused with both left where they were.
#[cfg(unix)]
#[test]
fn a_hard_link_differing_only_in_case_is_still_a_collision() {
    let (dir, file) = fixture();
    let upper = dir.path().join("SAMPLE.TXT");
    std::fs::hard_link(&file, &upper).unwrap();

    let err = host()
        .rename(&file, &upper)
        .expect_err("two names, not one");
    assert!(matches!(err, PlatformError::TargetExists { .. }), "{err:?}");
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["SAMPLE.TXT", "sample.txt"],
        "nothing left under a temporary name"
    );
}

// --- replace_file -----------------------------------------------------------

#[test]
fn replace_file_puts_the_new_contents_under_the_old_name() {
    let (dir, file) = fixture();
    let temp = dir.path().join(".sample.txt.tmp");
    std::fs::write(&temp, b"rewritten").unwrap();

    host().replace_file(&temp, &file).expect("replace");

    assert_eq!(std::fs::read(&file).unwrap(), b"rewritten");
    assert!(!temp.exists(), "the temporary name is gone");
}

/// It replaces; it never creates. Both platforms agree, because
/// `ReplaceFileW` cannot do otherwise.
#[test]
fn replace_file_refuses_a_target_that_is_not_there() {
    let dir = TempDir::new().unwrap();
    let temp = dir.path().join("new.tmp");
    std::fs::write(&temp, b"new").unwrap();

    host()
        .replace_file(&temp, &dir.path().join("missing.txt"))
        .expect_err("nothing to replace");
    assert_eq!(
        std::fs::read(&temp).unwrap(),
        b"new",
        "and the new file is untouched"
    );
    assert!(!dir.path().join("missing.txt").exists());
}

/// The reason `replace_file` is a platform call at all: on Windows the file
/// keeps its identity, created date included, through a rewrite.
#[cfg(windows)]
#[test]
fn replace_file_keeps_the_created_date_on_windows() {
    let (dir, file) = fixture();
    let platform = host();
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    platform
        .set_times(
            &file,
            TimeChange {
                created: Some(created),
                ..Default::default()
            },
        )
        .expect("set created");

    let temp = dir.path().join(".sample.txt.tmp");
    std::fs::write(&temp, b"rewritten").unwrap();
    platform.replace_file(&temp, &file).expect("replace");

    assert_eq!(std::fs::read(&file).unwrap(), b"rewritten");
    let got = platform.get_times(&file).unwrap().created.unwrap();
    let delta = got
        .duration_since(created)
        .or_else(|e| Ok::<_, std::time::SystemTimeError>(e.duration()))
        .unwrap();
    assert!(delta < Duration::from_secs(1), "created moved by {delta:?}");
}

// --- A link is the row, not its target ---------------------------------------

/// A listed symlink shows its own dates, so a date written to the row goes
/// on the link. Writing through it changed a file no row was about, and the
/// row then showed the old date anyway.
#[cfg(unix)]
#[test]
fn a_date_set_on_a_link_changes_the_link_and_not_its_target() {
    let (dir, target) = fixture();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let before = std::fs::metadata(&target).unwrap().modified().unwrap();
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);

    host()
        .set_times(
            &link,
            TimeChange {
                modified: Some(when),
                ..Default::default()
            },
        )
        .expect("set the link's date");

    assert_eq!(
        std::fs::metadata(&target).unwrap().modified().unwrap(),
        before
    );
    assert_eq!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .modified()
            .unwrap(),
        when
    );
    assert_eq!(host().get_times(&link).unwrap().modified, Some(when));
}

/// A dangling link is a real row since 1.4.0, and a date on it is a date on
/// the link — which exists, so it does not fail.
#[cfg(unix)]
#[test]
fn a_dangling_link_can_have_its_date_set() {
    let dir = TempDir::new().unwrap();
    let link = dir.path().join("dangling");
    std::os::unix::fs::symlink(dir.path().join("gone"), &link).unwrap();
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);

    host()
        .set_times(
            &link,
            TimeChange {
                accessed: Some(when),
                modified: Some(when),
                ..Default::default()
            },
        )
        .expect("the link exists even though its target does not");
    assert_eq!(host().get_times(&link).unwrap().modified, Some(when));
}

/// Linux has no `lchmod`, and `chmod` would change the target. So a
/// read-only request on a link is refused, and the target keeps its mode.
#[cfg(unix)]
#[test]
fn read_only_on_a_link_is_refused_rather_than_applied_to_its_target() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, target) = fixture();
    std::fs::set_permissions(&target, PermissionsExt::from_mode(0o644)).unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert!(!host().get_attributes(&link).unwrap().read_only);
    host()
        .set_attributes(
            &link,
            AttributeChange {
                read_only: Some(true),
                ..Default::default()
            },
        )
        .expect_err("there is no read-only bit on the link to set");
    let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644, "the target was not touched");

    // Asking for what the link already is changes nothing, so it is fine.
    host()
        .set_attributes(
            &link,
            AttributeChange {
                read_only: Some(false),
                ..Default::default()
            },
        )
        .expect("no change is no error");
}
