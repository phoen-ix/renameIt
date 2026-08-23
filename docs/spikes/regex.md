# Spike A — regex dialect verdict

*M0's regex spike. Written 2026-08-15. Executable half:
`crates/ren-core/tests/spike_regex.rs` (24 tests). Wrapper:
`crates/ren-core/src/regex_flavor.rs`.*

## Question

Find & Replace exposes regular expressions in the Microsoft JScript/VBScript
dialect, so that metacharacter reference is the contract. Does `fancy-regex`
(MIT, chosen in `docs/DESIGN.md` §6 for lookaround + backreference support)
reproduce it?

## Verdict

**Yes — adopt `fancy-regex`.** 33 of the 36 documented constructs behave
identically. Three deviations exist; all three are recorded below and none of
them can affect a filename. Two wrapper-level fixes were required and are
implemented.

Method: every row of the appendix table became a test case, with the Help
sentence quoted in the test. Ground truth for each case was measured against
`fancy-regex` 0.19.0 before the assertions were written, so the tests document
observed behaviour rather than assumed behaviour.

## Constructs verified identical

`\` (escape) · `^` · `$` · `*` · `+` · `?` · `{n}` · `{n,}` · `{n,m}` ·
non-greedy quantifier suffix · `.` (excludes `\n`) · the `[.\n]` idiom ·
`(pattern)` · `(?:pattern)` · `(?=pattern)` · `(?!pattern)` · `x|y` · `[xyz]` ·
`[^xyz]` · `[a-z]` · `[^a-z]` · `\b` · `\B` · `\d` · `\D` · `\f` · `\n` · `\r` ·
`\s` · `\S` · `\t` · `\v` · `\w` · `\W` · `\xnn` (exactly two digits, so
`\x041` is `\x04` then `1`, as documented) · `\num` backreferences ·
`$1`–`$9` in the replacement.

The worked example — `( don)[ \`´]?(t)` → `$1'$2` matching "dont", "don t",
"don\`t", "don´t" and preserving the source capitalisation — passes verbatim,
including the `DONT` → `DON'T` case.

## Deviations (recorded under P7)

| # | Construct | Behaviour | Impact |
|---|---|---|---|
| R1 | `\cx` control-character escape | **Not supported.** `fancy-regex` rejects it: *"Invalid escape: \c"*. | None. No platform we support permits control characters in a filename, so a pattern that matches one can never fire. |
| R2 | Octal escapes `\n` / `\nm` / `\nml` | **Not supported.** The *backreference* half of the documented rule works (`\1` is group 1); the *octal fallback* does not — `\7` with fewer than seven groups is a compile error rather than U+0007. | None, for the same reason as R1. `\xnn` remains available for byte values. |
| R3 | `\w \d \s` (and `\b`) character sets | **Widened.** JScript defines `\w` as `[A-Za-z0-9_]` and `\s` as `[ \f\n\r\t\v]`. Ours are Unicode-aware, so `\w+` matches `café` whole and `\d` matches Arabic-Indic digits. | Deliberate. A renamer that cannot see non-ASCII letters is not useful in 2026. `PatternOptions::unicode = false` restores exact JScript semantics if a per-op switch is ever wanted. |

There is also one **superset**: `fancy-regex` supports lookbehind (`(?<=…)`),
which JScript does not. Harmless, but it means a pattern written for RenameIt
may not run under a strict JScript engine.

## Wrapper fixes (both implemented in `regex_flavor.rs`)

### 1. `$1` followed by text silently expanded to nothing

Rust's replacement syntax reads `$` plus the *longest* run of word characters as
a group name. So `$1x` asks for a group called `1x`, finds none, and expands the
whole replacement to the empty string — silently. JScript reads exactly one
digit.

This is not academic: `$1_$2`, `$1v2`, `$12` are all things a user types, and
the failure mode is a filename becoming empty rather than an error.

`translate_replacement()` rewrites `$1`–`$9` into Rust's unambiguous `${1}`–
`${9}` before evaluation, escapes every other `$` to a literal, and keeps `$$`
as the escape hatch. `$1x` → `ax`, `$12` → `a2`, `cost: $` stays literal.

### 2. `replace_all` panics on backtrack-limit overflow

`fancy_regex::Regex::replace_all` is literally `try_replacen(..).unwrap()`. In a
preview that runs per file per keystroke, one pathological pattern would take
the whole application down. Everything goes through `try_replacen`, and
`RuntimeError::BacktrackLimitExceeded` is classified into
`RegexError::TooSlow { limit }` — the per-row "pattern too slow" error P7
promises.

## Bonus finding: catastrophic backtracking is much rarer than feared

`docs/DESIGN.md` lists "user patterns can be exponential" as a key risk. It is
smaller than it looks. `fancy-regex` delegates any sub-expression the
linear-time `regex` engine can handle, and that delegation is aggressive — the
textbook bombs are all handled in microseconds:

| Pattern | Subject | Result |
|---|---|---|
| `(a+)+$` | 40 × `a` + `!` | 29 µs, delegated |
| `(a\|aa)+(?=c)` | 40 × `a` + `b` | 105 µs, delegated |
| `^(?=(a+)+$)` | 40 × `a` + `!` | 15 µs, delegated |
| `(x+x+)+(?=y)` | 40 × `x` + `z` | 10 µs, delegated |
| `(a+)+\1b` | 30 × `a` + `c` | 97 µs, delegated |
| `(\w+\s?)*\1$` | a 38-char sentence | **backtrack limit exceeded** |

Only a backreference *to a repeated group* reaches the backtracking VM. The
budget still matters — that last row is exactly the case P7 exists for — but the
default was set to 100 000 steps (a tenth of `fancy-regex`'s own default)
because the trade-off is "abandon a rare hostile pattern" versus "stall the
preview", and it costs nothing on real patterns.

## Batch Replace default list

`crates/ren-core/data/batch_replace_default.toml` holds the shipped defaults
(**D6**: our data, our format). Rather than fifty near-identical regexes, one
per English contraction, it stores the linguistic pairs and generates the
pattern

```
find     ( <head>)[ `´]?(<tail>)·          replace  $1'$2·
```

Tests verify that all 50 pairs compile, that each repairs its own contraction
with every apostrophe stand-in (none, space, backtick, U+00B4), that
capitalisation is preserved, and that the rules do not fire inside longer words
("a pedant speaks" survives).

## Consequences for later milestones

- **M1 (Replace op):** wildcard mode (`*` = 0+, `:` = 1+, `?` = exactly 1)
  compiles to `.*`, `.+`, `.` with everything else escaped. Regex mode uses
  `Pattern` directly. `Skip`/`Count` map onto `replacen`'s limit — `Count = 0`
  means *unlimited*, which is also `replacen`'s convention.
- **M1 (Batch Replace):** the loader for the TOML above; decide whether to close
  the gaps in the shipped contraction coverage ("we're", "we'll", "let's" are
  missing) and record it.
- **Preview integration:** `RegexError::TooSlow` must surface as a per-row error
  badge, never as a dialog or a hang.

## Reproducing

```sh
cargo test -p ren-core --test spike_regex
```
