//! The include filter: deciding whether a file takes part at all.
//!
//! *"The include filter tests each
//! filename before renaming, to determine if it is to be renamed or not."*
//!
//! * *"The include filter will always test matches against the filename, but
//!   you can also tell it to test again the whole path and/or filename
//!   extension."*
//! * *"Exclude files matching / containg — This is the opposite of the include
//!   filter above. […] If run together, it is applied **after** the include
//!   filter."*
//!
//! The filter is per-step, and it sees the *current*
//! name: *"Preset items running before can thus modify the filename in a way
//! the the include filter in the next preset item reacts upon."*

use serde::{Deserialize, Serialize};

use crate::cache::Cached;
use crate::matcher::{MatchSpec, Matcher};
use crate::model::split_file_name;
use crate::regex_flavor::RegexError;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IncludeFilter {
    /// *"Only matching files will be processed by rename function."*
    pub include: Option<MatchSpec>,
    /// Applied after `include`.
    pub exclude: Option<MatchSpec>,
    /// Also test the full path, not just the name.
    pub whole_path: bool,
    /// Also test the extension. The stem is always tested.
    pub extension: bool,
    pub case_sensitive: bool,

    /// Both matchers, compiled together on first use — this runs once per file
    /// per keystroke.
    ///
    /// Deliberately **private**, which makes struct-update syntax
    /// (`IncludeFilter { case_sensitive: true, ..old }`) a compile error
    /// outside this module. That syntax would move the already-populated cache
    /// onto the new configuration and keep matching by the old rules — a silent
    /// wrong answer. Use the builder methods instead; each returns a value with
    /// a fresh cache.
    #[serde(skip)]
    compiled: Cached<Result<Compiled, RegexError>>,
}

#[derive(Debug)]
struct Compiled {
    include: Option<Matcher>,
    exclude: Option<Matcher>,
}

impl IncludeFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// *"Include only files matching / containing"*.
    pub fn including(mut self, spec: MatchSpec) -> Self {
        self.include = Some(spec);
        self.compiled = Cached::new();
        self
    }

    /// *"Exclude files matching / containg"*, applied after the include.
    pub fn excluding(mut self, spec: MatchSpec) -> Self {
        self.exclude = Some(spec);
        self.compiled = Cached::new();
        self
    }

    /// Also test the whole path, not just the name.
    pub fn testing_whole_path(mut self, yes: bool) -> Self {
        self.whole_path = yes;
        self
    }

    /// Also test the extension. The stem is always tested.
    pub fn testing_extension(mut self, yes: bool) -> Self {
        self.extension = yes;
        self
    }

    pub fn matching_case(mut self, yes: bool) -> Self {
        self.case_sensitive = yes;
        self.compiled = Cached::new();
        self
    }

    pub fn is_identity(&self) -> bool {
        self.include.is_none() && self.exclude.is_none()
    }

    /// Does this file take part?
    ///
    /// `path` is the full path; `file_name` is the *current* name, which may
    /// already differ from the path's last component because an earlier step
    /// changed it.
    fn compiled(&self) -> Result<&Compiled, RegexError> {
        self.compiled
            .get_or_init(|| {
                // Filter semantics: a plain string is "contains", a wildcard
                // string is a mask over the whole subject. See
                // MatchSpec::compile_for_filter.
                let compile = |spec: &Option<MatchSpec>| -> Result<Option<Matcher>, RegexError> {
                    spec.as_ref()
                        .map(|s| s.compile_for_filter(self.case_sensitive))
                        .transpose()
                };
                Ok(Compiled {
                    include: compile(&self.include)?,
                    exclude: compile(&self.exclude)?,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    pub fn accepts(&self, path: &str, file_name: &str) -> Result<bool, RegexError> {
        let compiled = self.compiled()?;
        if let Some(include) = &compiled.include
            && !self.matches_any(include, path, file_name)?
        {
            return Ok(false);
        }
        if let Some(exclude) = &compiled.exclude
            && self.matches_any(exclude, path, file_name)?
        {
            return Ok(false);
        }
        Ok(true)
    }

    /// True if any of the enabled subjects matches.
    fn matches_any(
        &self,
        matcher: &Matcher,
        path: &str,
        file_name: &str,
    ) -> Result<bool, RegexError> {
        let (stem, ext) = split_file_name(file_name);

        // The stem is always tested.
        if matcher.is_match(stem)? {
            return Ok(true);
        }
        if self.extension
            && let Some(ext) = ext
            && matcher.is_match(ext)?
        {
            return Ok(true);
        }
        if self.whole_path && matcher.is_match(path)? {
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepts(filter: &IncludeFilter, path: &str, name: &str) -> bool {
        filter.accepts(path, name).unwrap()
    }

    #[test]
    fn the_default_filter_accepts_everything() {
        let f = IncludeFilter::default();
        assert!(f.is_identity());
        assert!(accepts(&f, "/music/song.mp3", "song.mp3"));
    }

    /// "Only matching files will be processed by rename function."
    #[test]
    fn include_keeps_only_matching_files() {
        let f = IncludeFilter::new().including(MatchSpec::Substring("live".into()));
        assert!(accepts(&f, "/m/live set.mp3", "live set.mp3"));
        assert!(!accepts(&f, "/m/studio.mp3", "studio.mp3"));
    }

    /// "it is applied after the include filter, so you can tell the program to
    /// first include a certain file set, and then from this set exclude some of
    /// the files"
    #[test]
    fn exclude_runs_after_include() {
        let f = IncludeFilter::new()
            .including(MatchSpec::Substring("track".into()))
            .excluding(MatchSpec::Substring("demo".into()));
        assert!(accepts(&f, "/m/track 1.mp3", "track 1.mp3"));
        assert!(!accepts(&f, "/m/track demo.mp3", "track demo.mp3"));
        assert!(!accepts(&f, "/m/other.mp3", "other.mp3"));
    }

    #[test]
    fn exclude_alone_works_without_an_include() {
        let f = IncludeFilter::new().excluding(MatchSpec::Substring("tmp".into()));
        assert!(accepts(&f, "/a/keep.txt", "keep.txt"));
        assert!(!accepts(&f, "/a/tmp.txt", "tmp.txt"));
    }

    /// "The include filter will always test matches against the filename, but
    /// you can also tell it to test again the whole path and/or filename
    /// extension."
    #[test]
    fn the_extension_is_only_tested_when_asked() {
        let f = IncludeFilter::new().including(MatchSpec::Substring("mp3".into()));
        assert!(!accepts(&f, "/m/song.mp3", "song.mp3"));

        let f = f.testing_extension(true);
        assert!(accepts(&f, "/m/song.mp3", "song.mp3"));
    }

    #[test]
    fn the_whole_path_is_only_tested_when_asked() {
        let f = IncludeFilter::new().including(MatchSpec::Substring("Bootlegs".into()));
        assert!(!accepts(&f, "/music/Bootlegs/song.mp3", "song.mp3"));

        let f = f.testing_whole_path(true);
        assert!(accepts(&f, "/music/Bootlegs/song.mp3", "song.mp3"));
    }

    /// "You can also enter a wildcard string (eg "hello*") for more advanced
    /// matches" — which only means anything if the wildcard form is a mask over
    /// the whole subject rather than another substring search.
    #[test]
    fn a_wildcard_filter_is_a_mask_over_the_whole_name() {
        let f = IncludeFilter::new().including(MatchSpec::auto("track ?"));
        assert!(accepts(&f, "/m/track 1.mp3", "track 1.mp3"));
        assert!(!accepts(&f, "/m/track 10.mp3", "track 10.mp3"));
        assert!(!accepts(&f, "/m/my track 1.mp3", "my track 1.mp3"));
    }

    #[test]
    fn the_classic_file_mask_works_when_the_extension_is_tested() {
        let f = IncludeFilter::new()
            .including(MatchSpec::auto("*.mp3"))
            .testing_whole_path(true);
        assert!(accepts(&f, "/m/song.mp3", "song.mp3"));
        assert!(!accepts(&f, "/m/song.flac", "song.flac"));
    }

    #[test]
    fn filtering_is_case_insensitive_unless_asked() {
        let f = IncludeFilter::new().including(MatchSpec::Substring("LIVE".into()));
        assert!(accepts(&f, "/m/live.mp3", "live.mp3"));

        // The builder resets the compiled cache, so the new setting takes
        // effect even though the old filter had already matched something.
        let f = f.matching_case(true);
        assert!(!accepts(&f, "/m/live.mp3", "live.mp3"));
    }
}
