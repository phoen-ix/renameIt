//! Filesystem naming rules as *data*.
//!
//! Adding a platform (D3: Linux alpha in M9, macOS later) means adding a table
//! here, not writing code. The planner uses [`NamingRules::validate_component`]
//! for `InvalidName` conflicts and [`NamingRules::fold`] to build the
//! case-folded target map that detects duplicates.

use std::fmt;

/// What [`NamingRules::max_component_len`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthUnit {
    /// NTFS and friends: 255 UTF-16 code units.
    Utf16Units,
    /// Most POSIX filesystems: 255 bytes of the encoded name.
    Bytes,
}

/// Why a filename component is not writable on this platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    /// `.` or `..` — path components with special meaning, never file names.
    DotComponent,
    IllegalChar(char),
    ControlChar(char),
    /// A reserved device name such as `CON` or `LPT1`, with or without an extension.
    ReservedName(String),
    TrailingDot,
    TrailingSpace,
    TooLong {
        len: usize,
        max: usize,
    },
}

impl fmt::Display for NameProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "name is empty"),
            Self::DotComponent => write!(f, "\".\" and \"..\" are not file names"),
            Self::IllegalChar(c) => write!(f, "character {c:?} is not allowed in a filename"),
            Self::ControlChar(c) => {
                write!(f, "control character U+{:04X} is not allowed", *c as u32)
            }
            Self::ReservedName(n) => write!(f, "{n} is a reserved device name"),
            Self::TrailingDot => write!(f, "name may not end with a period"),
            Self::TrailingSpace => write!(f, "name may not end with a space"),
            Self::TooLong { len, max } => write!(f, "name is {len} long, the limit is {max}"),
        }
    }
}

/// The rule table for one filesystem family.
#[derive(Debug, Clone, Copy)]
pub struct NamingRules {
    pub id: &'static str,
    pub illegal_chars: &'static [char],
    pub forbid_control_chars: bool,
    /// Matched case-insensitively against the part before the first period.
    pub reserved_stems: &'static [&'static str],
    pub forbid_trailing_dot: bool,
    pub forbid_trailing_space: bool,
    pub max_component_len: usize,
    pub length_unit: LengthUnit,
    /// Whether names that differ only in case collide.
    pub case_insensitive: bool,
}

impl NamingRules {
    /// Length of `name` in whatever unit this filesystem counts.
    pub fn component_len(&self, name: &str) -> usize {
        match self.length_unit {
            LengthUnit::Utf16Units => name.encode_utf16().count(),
            LengthUnit::Bytes => name.len(),
        }
    }

    /// Normalised key for the planner's duplicate-target map.
    pub fn fold(&self, name: &str) -> String {
        if self.case_insensitive {
            name.to_lowercase()
        } else {
            name.to_owned()
        }
    }

    /// Checks a single path component (a file or folder name, never a path).
    pub fn validate_component(&self, name: &str) -> Result<(), NameProblem> {
        if name.is_empty() {
            return Err(NameProblem::Empty);
        }
        // Universal, not per-filesystem: `.` and `..` mean "this directory" and
        // "the parent" everywhere. A rename onto either is never what the user
        // meant, and on a good day it merely fails. Space Trimming turns the
        // real filename ". ." into ".." — found by the M1 property tests.
        if name == "." || name == ".." {
            return Err(NameProblem::DotComponent);
        }
        for c in name.chars() {
            if self.forbid_control_chars && (c as u32) < 0x20 {
                return Err(NameProblem::ControlChar(c));
            }
            if self.illegal_chars.contains(&c) {
                return Err(NameProblem::IllegalChar(c));
            }
        }
        if self.forbid_trailing_dot && name.ends_with('.') {
            return Err(NameProblem::TrailingDot);
        }
        if self.forbid_trailing_space && name.ends_with(' ') {
            return Err(NameProblem::TrailingSpace);
        }
        if !self.reserved_stems.is_empty() {
            let stem = name.split('.').next().unwrap_or(name);
            if self
                .reserved_stems
                .iter()
                .any(|r| r.eq_ignore_ascii_case(stem))
            {
                return Err(NameProblem::ReservedName(stem.to_owned()));
            }
        }
        let len = self.component_len(name);
        if len > self.max_component_len {
            return Err(NameProblem::TooLong {
                len,
                max: self.max_component_len,
            });
        }
        Ok(())
    }
}

const WINDOWS_ILLEGAL: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// `CON.txt` is just as reserved as `CON`, which is why these are stems.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub static WINDOWS: NamingRules = NamingRules {
    id: "windows",
    illegal_chars: WINDOWS_ILLEGAL,
    forbid_control_chars: true,
    reserved_stems: WINDOWS_RESERVED,
    forbid_trailing_dot: true,
    forbid_trailing_space: true,
    max_component_len: 255,
    length_unit: LengthUnit::Utf16Units,
    case_insensitive: true,
};

pub static POSIX: NamingRules = NamingRules {
    id: "posix",
    illegal_chars: &['/'],
    forbid_control_chars: false,
    reserved_stems: &[],
    forbid_trailing_dot: false,
    forbid_trailing_space: false,
    max_component_len: 255,
    length_unit: LengthUnit::Bytes,
    case_insensitive: false,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_rejects_the_documented_illegal_characters() {
        for c in WINDOWS_ILLEGAL {
            let name = format!("a{c}b.txt");
            assert_eq!(
                WINDOWS.validate_component(&name),
                Err(NameProblem::IllegalChar(*c)),
                "{name} should be rejected"
            );
        }
    }

    #[test]
    fn windows_rejects_reserved_device_names_with_and_without_extension() {
        for name in ["CON", "con", "Con.txt", "LPT9.tar.gz", "NUL"] {
            assert!(
                matches!(
                    WINDOWS.validate_component(name),
                    Err(NameProblem::ReservedName(_))
                ),
                "{name} should be reserved"
            );
        }
        assert!(WINDOWS.validate_component("CONSOLE.txt").is_ok());
        assert!(WINDOWS.validate_component("COM10.txt").is_ok());
    }

    #[test]
    fn windows_rejects_trailing_dot_and_space() {
        assert_eq!(
            WINDOWS.validate_component("report."),
            Err(NameProblem::TrailingDot)
        );
        assert_eq!(
            WINDOWS.validate_component("report "),
            Err(NameProblem::TrailingSpace)
        );
    }

    /// A rename onto `.` or `..` targets a directory, not a file. Space
    /// Trimming reaches it from the perfectly ordinary filename ". ." — which
    /// is how the property tests found this.
    #[test]
    fn dot_and_dot_dot_are_never_valid_names_on_any_platform() {
        for rules in [&WINDOWS, &POSIX] {
            assert_eq!(
                rules.validate_component("."),
                Err(NameProblem::DotComponent),
                "{}",
                rules.id
            );
            assert_eq!(
                rules.validate_component(".."),
                Err(NameProblem::DotComponent),
                "{}",
                rules.id
            );
        }
        // Longer runs of dots are merely odd, not special.
        assert!(POSIX.validate_component("...").is_ok());
        assert!(POSIX.validate_component(".hidden").is_ok());
    }

    #[test]
    fn posix_only_rejects_the_separator_and_empty_names() {
        assert_eq!(POSIX.validate_component(""), Err(NameProblem::Empty));
        assert_eq!(
            POSIX.validate_component("a/b"),
            Err(NameProblem::IllegalChar('/'))
        );
        // Perfectly legal on POSIX, catastrophic on Windows.
        assert!(POSIX.validate_component("CON").is_ok());
        assert!(POSIX.validate_component("what?.txt").is_ok());
        assert!(POSIX.validate_component("trailing. ").is_ok());
    }

    #[test]
    fn length_is_counted_in_the_filesystems_own_unit() {
        // "é" is 1 UTF-16 unit but 2 UTF-8 bytes.
        let name = "é".repeat(200);
        assert_eq!(WINDOWS.component_len(&name), 200);
        assert_eq!(POSIX.component_len(&name), 400);
        assert!(WINDOWS.validate_component(&name).is_ok());
        assert!(matches!(
            POSIX.validate_component(&name),
            Err(NameProblem::TooLong { .. })
        ));
    }

    #[test]
    fn folding_collapses_case_only_on_case_insensitive_filesystems() {
        assert_eq!(WINDOWS.fold("ReadMe.TXT"), WINDOWS.fold("readme.txt"));
        assert_ne!(POSIX.fold("ReadMe.TXT"), POSIX.fold("readme.txt"));
    }
}
