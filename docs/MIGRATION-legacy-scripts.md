# Porting a `.frs` script

Legacy `.frs` scripts are VBScript, hosted by a Windows-only COM scripting
component. RenameIt is cross-platform and MIT-licensed, and neither of those
survives embedding such a host, so **`.frs` files do not run**. The language is [Koto](https://koto.dev), the files are `.koto`, and this
page is the translation.

It is a real break, and worth being blunt about: a script you wrote for the
legacy host has to be rewritten, not converted. The *shape* is unchanged —
the same lifecycle, the same eleven members (and four new ones), the same
Script folder and Arguments box — so the rewrite is usually mechanical. All nine shipped examples
were written this way, and they are in your script folder.

Your `.frs` files are not deleted or hidden. They show in the script picker,
greyed, with their own description, pointing here.

---

## The one thing that will catch you

**Koto captures scalars by value.** This is the only difference that fails
*silently*, so it comes first.

VBScript's script-level variables are static: they keep their value for the
entire rename session, and three of the shipped examples rely on that. In Koto,
a function that closes over a number or a string gets a **copy**:

```koto
# WRONG — every file sees 1
n = 0
rename = ||
  n += 1        # writes to this call's copy
  '{n}'
```

No error, no warning, a counter that never counts. Maps and lists are reference
types and do carry state, so session state lives in a map:

```koto
# RIGHT
state = {n: 0}
rename = ||
  state.n += 1
  '{state.n}'
```

If your script accumulates anything across files, put it in a map.

---

## The file

| | Legacy `.frs` | Ours `.koto` |
|---|---|---|
| Line 1 | `language=vbscript` | *(gone — there is one language)* |
| Line 2 | `description=…` | `# description: …` |
| Argument hint | buried in the description prose | `# args: …` *(new)* |
| Encoding | ANSI / CP1252 | UTF-8 |

```koto
# description: Replace non-English characters with a safe base version.
# args: none

rename = ||
  fr.filename
```

Both header lines are optional, and only the leading run of comments is scanned
— a `# description:` further down the file is just a comment.

`# args:` has no legacy counterpart — a `.frs` file documents its argument
syntax in free prose inside `description=`. Omit it and the card says the script
takes no arguments; give it and the text appears beside the Arguments box.

---

## The lifecycle

| Legacy | Ours |
|---|---|
| `Function Init()` | **top-level code** — it runs once, before the first file |
| `Function Rename()` | `rename = \|\| …` |
| `Function Done()` | `done = \|\| …` |

`init` has no separate existence: top-level code *is* the initialisation step,
because Koto runs the file. Anything `Init()` did goes at the top of the file.

`rename` is required. `done` is optional. A run is one session: a session starts
when you press the rename button, or when a new preview is performed.

```vbscript
' Legacy
Dim iCount
Function Init()
    iCount = 0
End Function
Function Rename()
    iCount = iCount + 1
    Rename = "File " & iCount
End Function
```

```koto
# Ours
state = {count: 0}

rename = ||
  state.count += 1
  'File {state.count}'
```

**Returning a value.** VBScript assigns to the function name; Koto returns the
last expression. Returning an empty string still skips the file — `''` means
leave this file alone. A number is accepted and turned
into a string, so `Length of Filename`'s
`rename = || fr.full_filename.chars().count()` works as it reads.

**What you return is obeyed like typed text.** A `/` (or `\`) in it moves the
file into a subfolder, exactly as it would in a Free Format field. That is
right when the script means it and wrong when the text came out of a file: a
page titled `HTTP/2` would land in a folder called `HTTP`. The built-in tags
that read a file's contents turn separators into `-`, and a script that takes
a name from `fr.contents()`, `fr.args_file()` or `fr.format_tags` should do
the same — `Get HTML XML Tags` ends with `.replace('/', '-').replace('\\', '-')`.

---

## The `fr` object

The script object is `fr`, and the member names are snake_case. Everything else
is the same value the legacy object carried. It is an ordinary map: a script
can assign to a member, and nothing the engine does reads the change back.

| Legacy member | Ours | Notes |
|---|---|---|
| `.Filename` | `fr.filename` | still depends on the card's Scope |
| `.FullFilename` | `fr.full_filename` | |
| `.Path` | `fr.path` | still ends with a separator |
| `.DiskName` | `fr.disk_name` | |
| `.Args` | `fr.args` | the Arguments box — see below |
| `.Preview` | `fr.preview` | **always `true`** — see below |
| `.NumItems` | `fr.num_items` | |
| `.ItemOrder` | `fr.item_order` | 0-based, zero-padded |
| `.BrowserPath` | `fr.browser_path` | empty in free select, and whenever the files span more than one folder (subfolders listed) |
| `.FormatTags("<size>")` | `fr.format_tags '<size>'` | the real tag engine |
| `.GetAllFilenames` | `fr.all_filenames()` | a **List**, not a `*`-joined string |

Three of these need more than a row.

**`fr.preview` is always `true`, and that is a guarantee rather than a
limitation.** A script only ever runs while the rename is being *planned*; the
executor replays a finished plan and never re-evaluates one. So a preview
cannot write anything, rather than every script having to remember to check the
flag itself. Your `If Not .Preview Then` guards should come out — the code
inside them will never run. If the guard protected a *file
write*, see the next section.

**Keep `fr.args` as it came.** A legacy script could assign to it
(`.Args = trim(.Args)`), and so can a Koto one — `fr` is a map — but a trimmed
copy in a local says what it means and cannot surprise a helper that reads
`fr.args` later:

```koto
tag = fr.args.trim()
```

`fr.args_file()` reads the path that was in the Arguments box when the run
started, whatever `fr.args` has been set to since.

**`fr.all_filenames()` returns a List.** The legacy member returned one string
with `*` between the names, which every caller then had to split. If you want
the old shape: `fr.all_filenames().intersperse('*').to_string()`.

### Four members with no legacy counterpart

Each exists because something a legacy script did through unrestricted COM has
to be done some other way once the filesystem is closed off.

| Ours | What it gives you | Replaces |
|---|---|---|
| `fr.contents()` | the text of **the file being renamed**, or `''` | `FileSystemObject.OpenTextFile(…).ReadAll` |
| `fr.args_file()` | the text of the file named in the Arguments box | the same, for a data file |
| `fr.seed` | the run's random seed, an integer | `Randomize` |
| `fr.find_ci(h, n)` | case-insensitive index-of, or `null` | `InStr(…, vbTextCompare)` |

**Do not fold a string and then index the unfolded one with the offsets.** It is
the obvious way to search without an index-of, and it is wrong: `to_lowercase`
can change a string's byte length, so every offset past such a character is
off. `fr.find_ci` searches the string you hand it and returns an offset valid
for that string.

`fr.contents()` and `fr.args_file()` take **no path** — that is the point. One
reads the current file, the other reads the one path the user typed into this
card, and neither can be pointed anywhere else. Both cap at 5 MB, which is the
same limit `Get HTML XML Tags.frs` set for itself, and both decode leniently so
a CP1252 file still yields its ASCII content rather than failing.

`fr.seed` is stable for a run and different between runs, which is what makes a
scripted preview agree with the rename that follows it. `Unique Random
Number.koto` seeds a small LCG from it.

---

## Writing a file

A legacy script could do anything the user can, through
`CreateObject("Scripting.FileSystemObject")` — read, write, delete, anywhere.
No sandbox, and a preview running the same code as the rename.

Scripts here cannot touch the filesystem at all. Instead, `done` may **ask** for
one file to be written, by returning a map:

```koto
done = ||
  { path: fr.browser_path + 'Playlist.m3u'
  , contents: lines.intersperse('\n').to_string()
  , log: 'Created the playlist'      # optional
  }
```

The engine turns that into a planned operation, so the write is in the preview
and journalled like everything else.

- `path` must be **absolute**, and in **a folder the run lists** — the folder
  of a file being renamed. Build it from `fr.path` or `fr.browser_path`. A
  path anywhere else refuses the whole run, with a reason naming the path:
  `done` may be code somebody else wrote, and a write that could name any
  folder would hand back what removing `io` took away.
- It may not be the new name of a file the run renames, a file the run
  renames away, a folder, or a link. Each of those refuses the run too.
- The app asks before any run that writes a file, and lists each write.
  Creating a file that was not there can be undone; **overwriting one
  cannot**. On the command line a create needs nothing extra and an overwrite
  needs `--allow-irreversible`.

Anything else `done` returns is a line for the log.

`Create Mp3 Playlist.koto` is the worked example.

**Reading** is narrower rather than absent: `fr.contents()` gives you the file
being renamed and `fr.args_file()` the one path the user typed into the card.
Neither takes a path, so a script cannot read anything the user did not already
point it at. `Get HTML XML Tags` and `CSV List Rename` are the worked examples.

---

## Statements and functions

Koto is expression-oriented and indentation-significant. The common
substitutions:

| VBScript | Koto |
|---|---|
| `a & b` | `a + b`, or `'{a}{b}'` |
| `Len(s)` | `s.chars().count()` |
| `Mid(s, i)` / `Mid(s, i, n)` | `s.chars().skip(i - 1).to_string()` / `s.chars().skip(i - 1).take(n).to_string()` |
| `Left(s, n)` / `Right(s, n)` | `s.chars().take(n).to_string()` / `s.chars().skip(s.chars().count() - n).to_string()` |
| `InStr(1, h, n)` | `h.find n`, or `fr.find_ci h, n` for a case-insensitive one |
| `Replace(s, a, b)` | `s.replace a, b` |
| `LCase` / `UCase` | `.to_lowercase()` / `.to_uppercase()` |
| `Trim` | `.trim()` |
| `Split(s, ",")` | `s.split(',').to_tuple()` |
| `CStr(n)` / `CInt(s)` | `'{n}'` / `s.to_number()` |
| `Asc(c)` / `Chr(n)` | — / — |
| `If … Then … End If` | `if …` + indentation |
| `For i = 0 To n` | `for i in 0..=n` |
| `Dim a(10)` | `a = []`, then `a.push x` |
| `MsgBox "…"` | *(no UI — return an error or a log line)* |
| `Exit Function` | `return` |

**`size s` and `s[a..b]` count bytes, not characters.** A Koto string is
UTF-8, so `size 'Björk'` is 6 and `'Ärger'[0..1]` is an error — it would cut
the `Ä` in half. The VBScript functions count characters, which is what the
`chars()` forms above do; use `size` and byte slices only on text you know is
ASCII, or on offsets a search gave you (`fr.find_ci` returns byte offsets for
exactly that reason). `Asc` has no equivalent at all: `c.bytes().next()` is
the first UTF-8 byte, not the character's code.

Two that have no direct equivalent:

- **`MsgBox`** — a script has no way to open a dialog. Return an error from
  `rename` instead, which lands on the row where the user is already looking.
- **`Randomize`** — there is no clock. A bare `Randomize` is why a script would
  show one set of numbers in the preview and write a different set to disk.
  Seed from `fr.seed` instead; `Unique Random Number.koto` carries a five-line
  generator you can copy.

---

## Limits

A script's time is wall-clock, because that is the only budget Koto offers:

| Call | Budget | If it runs out |
|---|---|---|
| each `rename` | **10 ms** | that one row fails |
| the top level (init) | **1 s** | the script fails, on every row |
| `done` | **1 s** | nothing is written; the log says why |

The per-row budget exists because the preview re-runs on every keystroke. The
top level and `done` run once per session, and are where whole-listing work
belongs — loading a table into a map, building a playlist — so they get more.

The limit is checked *between* the script's own instructions, so it cannot
interrupt a single long call into Koto's library. Everything that could make
one is removed, refused, or bounded instead (D106) — you will get "not found"
for a removed function, and an error on the row for the rest:

| Removed | Why | Instead |
|---|---|---|
| `iterator.repeat`, `iterator.cycle`, `iterator.generate` | infinite, and outside the limit's reach | build a list with `push`, or iterate a range |
| `string.repeat` | `'x'.repeat 1e18` aborts the process | build it in a loop |
| `list.resize`, `list.resize_with`, `list.fill` | same | `push` |
| `koto.deep_copy` | a list that contains itself overflows the stack | `koto.copy`, or build the copy you need |

| Refused | Why | Instead |
|---|---|---|
| a range or iterator of more than a million items handed to a library function (`(0..1e9).count()`, `list.extend 0..1e9`) | the library walks it in one uninterruptible loop | a `for` loop, which the limit does reach |
| a generator (`yield`), or a map with its own `@next`, `@next_back` or `@iterator` | every item re-enters the script with a fresh limit, so a loop over an endless one never ends | a list |
| a format spec wider or more precise than 1024 (`'{x:4000000000}'`) | one allocation of that size | pad in a loop |
| nesting more than 100 deep, or a script over 64 KB | the parser recurses once per level, and running out of stack ends the process — the limits sit far below that | flatter code |

A library call that calls back into your script (`each`, `keep`, `fold`,
`sort` …) is held to the budget of the call it is part of: once that is spent,
the next callback is not made, and the row fails as slow like any other.

There is no `import`, and no way to load code at run time. A script is one file.

---

## The nine, for reference

Nine worked examples ship in your script folder as `.koto` files, each with
comments naming the edges it deliberately keeps:

| Script | Worth reading for |
|---|---|
| `Example - Base for New Script` | the shape, start here |
| `Swap Around` | argument handling; the shortest of the nine |
| `Insert Space Before Caps` | character-by-character work |
| `Safe Characters` | a large translation table |
| `Length of Filename` | returning a number; a script that renames nothing |
| `Get HTML XML Tags` | reading file content through the tag engine |
| `CSV List Rename` | loading data in `init` |
| `Unique Random Number` | session state in a map, and seeding |
| `Create Mp3 Playlist` | `done` writing a file; sorting by `fr.item_order` |
