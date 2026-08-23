# Porting a `.frs` script

Legacy `.frs` scripts are VBScript, hosted by a Windows-only COM scripting
component. RenameIt is cross-platform and MIT-licensed, and neither of those
survives embedding such a host, so **`.frs` files do not run**. The language is [Koto](https://koto.dev), the files are `.koto`, and this
page is the translation.

It is a real break, and worth being blunt about: a script you wrote for the
legacy script has to be rewritten, not converted. The *shape* is unchanged —
the same lifecycle, the same eleven members, the same Script folder and
Arguments box — so the rewrite is usually mechanical. All nine shipped examples
were written this way, and they are in your script folder.

Your `.frs` files are not deleted or hidden. They show in the script picker,
greyed, with their original description, pointing here.

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

| | Original `.frs` | Ours `.koto` |
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

| Original | Ours |
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
into a string, so `Length of Filename`'s `rename = || size fr.full_filename`
still works.

---

## The `fr` object

The script object is `fr`, and the member names are snake_case. Everything else
is the same value the legacy object carried.

| Legacy member | Ours | Notes |
|---|---|---|
| `.Filename` | `fr.filename` | still depends on the card's Scope |
| `.FullFilename` | `fr.full_filename` | |
| `.Path` | `fr.path` | still ends with a separator |
| `.DiskName` | `fr.disk_name` | |
| `.Args` | `fr.args` | **read-only, genuinely** — see below |
| `.Preview` | `fr.preview` | **always `true`** — see below |
| `.NumItems` | `fr.num_items` | |
| `.ItemOrder` | `fr.item_order` | 0-based, zero-padded |
| `.BrowserPath` | `fr.browser_path` | empty in free select |
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

**`fr.args` really is read-only.** A legacy script could assign to it
(`.Args = trim(.Args)`) despite the object being documented as read-only. Trim
it into a local instead:

```koto
tag = fr.args.trim()
```

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

The engine turns that into a planned operation, so the write is previewed,
confirmed, journalled and undoable like everything else — and creating a file
that was not there can be undone, while **overwriting one cannot**, and asks
first. `path` must be absolute; build it from `fr.path` or `fr.browser_path`.

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
| `Len(s)` | `size s` |
| `Mid(s, i)` / `Mid(s, i, n)` | `s[i-1..]` / `s[i-1..i-1+n]` — **0-based** |
| `Left(s, n)` / `Right(s, n)` | `s[..n]` / `s[(size s) - n..]` |
| `InStr(1, h, n)` | `h.find n`, or `fr.find_ci h, n` for a case-insensitive one |
| `Replace(s, a, b)` | `s.replace a, b` |
| `LCase` / `UCase` | `.to_lowercase()` / `.to_uppercase()` |
| `Trim` | `.trim()` |
| `Split(s, ",")` | `s.split(',').to_tuple()` |
| `CStr(n)` / `CInt(s)` | `'{n}'` / `s.to_number()` |
| `Asc(c)` / `Chr(n)` | `c.bytes().next()` / — |
| `If … Then … End If` | `if …` + indentation |
| `For i = 0 To n` | `for i in 0..=n` |
| `Dim a(10)` | `a = []`, then `a.push x` |
| `MsgBox "…"` | *(no UI — return an error or a log line)* |
| `Exit Function` | `return` |

Two that have no direct equivalent:

- **`MsgBox`** — a script has no way to open a dialog. Return an error from
  `rename` instead, which lands on the row where the user is already looking.
- **`Randomize`** — there is no clock. A bare `Randomize` is why a script would
  show one set of numbers in the preview and write a different set to disk.
  Seed from `fr.seed` instead; `Unique Random Number.koto` carries a five-line
  generator you can copy.

---

## Limits

A script gets **10 ms per file**. It is wall-clock, because that is the only
budget Koto offers, and it exists because the preview re-runs on every
keystroke. A script that overruns fails that one row.

The deadline is checked *between* VM instructions, so it cannot interrupt a
single long call into the runtime. A handful of functions are therefore removed
rather than policed (D106) — you will get "not found" if you reach for one:

| Removed | Why | Instead |
|---|---|---|
| `iterator.repeat`, `iterator.cycle`, `iterator.generate` | infinite, and outside the deadline's reach | build a list with `push`, or iterate a range |
| `string.repeat` | `'x'.repeat 1e18` aborts the process | build it in a loop |
| `list.resize`, `list.resize_with`, `list.fill` | same | `push` |

A script may also not nest more than 256 deep — koto's parser overflows its
stack long before that and takes the process with it.

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
