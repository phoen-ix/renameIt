# Tag reference

Any field that builds text takes `<tags>`. A tag is everything between a `<`
and the next `>`; everything else is literal text.

**A tag you mistype is an error, not an empty string** (D29). The editor names
it while you are typing, rather than letting one silent typo run across a
thousand files.

Tag names are matched case-insensitively and render back in the spelling the
tag menu shows. Where a tag takes a number it is written after a hyphen —
`<Left-3>`, `<Mid-2-5>`, `<Rnd3-1-100>`. A tag that takes an argument it cannot
use (`<Left-x>`, `<DirFiles-3>`, `<Time-Hh>`, an empty `<Date->`) is an error
too, and the message says what the tag wants.

## Name

| Tag | Gives |
|---|---|
| `<Name>` | Current filename |
| `<Ext>` | Current extension |
| `<FullName>` | Current full filename |
| `<Parent>` | Parent folder name |
| `<Parent-2>` | N-th parent folder name |
| `<Left-3>` | Left # characters |
| `<Right-3>` | Right # characters |
| `<Mid-2>` | Characters from position # to the end |
| `<Mid-2-5>` | Characters from position #1 to #2 |
| `<MidRev-3>` | Characters from position #, counting backwards |
| `<MidRev-5-2>` | Characters between two positions, counting backwards |
| `<FLetter>` | First letter in the name |
| `<FLetterN1>` | First letter, numbers included |
| `<FLetterN2>` | First letter, numbers included and substituted |

## Size

| Tag | Gives |
|---|---|
| `<Size>` | File size, scaled unit |
| `<SizeB>` | File size in bytes |
| `<SizeB-8>` | File size in bytes, zero-padded to # digits (`00001536`) |

## Folder

For a file row this is the folder the file is in; for a folder row it is the
folder itself. An `S` prefix means "and subfolders".

| Tag | Gives |
|---|---|
| `<DirFiles>` | Number of files in the folder |
| `<DirDirs>` | Number of subfolders |
| `<SDirFiles>` | Number of files, through subfolders |
| `<SDirDirs>` | Number of folders, through subfolders |
| `<DirSize>` | Size of the files in the folder, scaled unit |
| `<DirSizeB>` | Size of the files in the folder, in bytes |
| `<DirSizeB-8>` | The same, zero-padded to # digits |
| `<SDirSize>` | Size through subfolders, scaled unit |
| `<SDirSizeB>`, `<SDirSizeB-8>` | Size through subfolders, in bytes |
| `<FirstFileInFolder>` | Name of the first file in the folder |
| `<FirstFileInFolder-Ext>` | The same, with its extension |

`<DirLength>`, the playing time of the music in a folder, is not built (D129).

## Date and time

| Tag | Gives |
|---|---|
| `<Date>` / `<Time>` | Modified date / time |
| `<CDate>` / `<CTime>` | Created date / time |
| `<ADate>` / `<ATime>` | Accessed date / time |
| `<NowDate>` / `<NowTime>` | Date / time at the start of the run |
| `<Date-yyyy-mm-dd>` | Modified, in a format of your own |
| `<CDate-…>`, `<ADate-…>`, `<NowDate-…>` | The same for the other dates |

The default formats are `yyyy-mm-dd` and `Hh.Mm.Ss` — ISO 8601 order, so a
listing sorts correctly by name. A time is a format too: the time tags take no
format of their own, so write `<Date-Hh.Nn>` rather than `<Time-Hh.Nn>`.

### Format codes

| Code | Gives, for Monday 9 May 1977, 10:18:05 |
|---|---|
| `d` / `dd` | Day of the month: `9` / `09` |
| `ddd` / `dddd` | Weekday: `Mon` / `Monday` |
| `ddddd` | The short date, `1977-05-09` |
| `dddddd` | The long date, `Monday 9 May 1977` |
| `w` | Weekday as a number, Sunday = 1: `2` |
| `ww` | Week of the year, weeks starting on Sunday and the week holding 1 January as week 1: `20` |
| `m` / `mm` | Month: `5` / `05` — but a *minute* after `h` or `Hh` |
| `mmm` / `mmmm` | Month name: `May` / `May` |
| `q` | Quarter: `2` |
| `y` | **Day of the year**, not the year: `129` |
| `yy` / `yyyy` | Year: `77` / `1977` |
| `h` / `Hh` | Hour: `10` / `10` (zero-padded) |
| `N` / `Nn` | Minute: `18` / `18` — capital `N` only |
| `S` / `Ss` | Second: `5` / `05` |
| `ttttt` | The time, `10.18.05` |
| `c` | Date and time, `1977-05-09 10.18.05` |
| `AM/PM`, `am/pm`, `A/P`, `a/p`, `AMPM` | Morning or afternoon; with one of these, hours count 1–12 |
| `:` / `/` | Time and date separators, which render as `.` and `-` |

Named formats stand in for a whole format: `Short Date` (`yyyy-mm-dd`),
`Medium Date` (`d mmm yyyy`), `Long Date` (`dddd d mmmm yyyy`), `Short Time`
(`Hh.Mm`), `Medium Time` (`Hh.Mm AM/PM`), `Long Time` (`Hh.Mm.Ss`) and
`General Date` (`yyyy-mm-dd Hh.Mm.Ss`). They render the same on every machine,
whatever its regional settings (D30).

Codes are matched case-insensitively, except `N` and the AM/PM designators, so
**any letter that is a code needs a `\` in front of it to stay a letter**:
`<Date-Shot yyyy>` reads the `S` as seconds and the `h` as the hour, where
`<Date-\S\hot yyyy>` gives `Shot 1977`. An escaped `\/`, `\:` or `\\` still
renders as `-`, `.` or `-`, so a date can never put a path separator or a
character Windows refuses into a name.

A date tag with an empty format (`<Date->`) is an error rather than a date that
silently renders as nothing.

## Numbers

| Tag | Gives |
|---|---|
| `<Counter>` | The run counter (start, step, padding set in Counter Setup) |
| `<NumFiles>` | Number of items in the run |
| `<Rnd>` | Random character, A–Z |
| `<Rnd3>` | Random number, 0–9, # digits |
| `<Rnd3-1-100>` | Random number in a custom range; either end may be negative, as in `<Rnd3--5-5>` |
| `<RndZ3-1-100>` | The same, zero-padded to # digits |

The random seed is chosen once per session (D107) — when the app starts, on
Reset, and when a preset loads — so a preview and the rename that follows it
produce the same numbers (P16). It also means the same row gets the same number
in two runs of one session. A job file pins it with `seed` under `[settings]`,
and `ren-cli --seed` does the same, to reproduce a run exactly.

## Music

| Tag | Gives |
|---|---|
| `<Artist>`, `<Title>`, `<Album>`, `<Year>`, `<Genre>`, `<Comment>` | The common tags |
| `<Track>` / `<TrackN>` | Track number, zero-padded / not padded |
| `<Length>` / `<LengthS>` | Length, long / short form |
| `<Bitrate>` | Bitrate in kbps |
| `<Stereo>` | "Stereo" or "Mono" |
| `<Freq>` / `<FreqS>` | Frequency in Hz / kHz |
| `<ID3-*>` | About fifty named frames — `<ID3-AlbumArtist>`, `<ID3-Composer>`, `<ID3-DiscNumber>`, `<ID3-Publisher>`, `<ID3-Lyrics>` and the rest |

These read from MP3, FLAC, Ogg Vorbis, Opus, Speex, MP4/M4A, Musepack and
WavPack — one set of names across every format. WMA and TTA are not supported
(D55).

A **folder** answers with the tags of the first music file inside it.

## Image

| Tag | Gives |
|---|---|
| `<ExifDate>` / `<ExifTime>` | When the photograph was taken |
| `<Exif-Make>`, `<Exif-Model>` | Camera manufacturer and model |
| `<Exif-PixelXDimension>`, `<Exif-PixelYDimension>` | Image width and height |
| `<Exif-FNumber>`, `<Exif-ExposureTime>` | Aperture, shutter speed |
| `<Exif-PhotographicSensitivity>` | ISO speed |
| `<Exif-FocalLength>`, `<Exif-Orientation>` | Focal length, orientation |
| `<ExifDate-…>` | The date taken, in a format of your own |
| `<Width>`, `<Height>` | Pixel dimensions from the image header |
| `<Depth>`, `<Depthb>` | Colour depth, in colours / in bits |
| `<JpgComment>` | The comment stored inside a JPEG |

`<Exif-*>` takes the field names the Exif standard gives — `ExposureBiasValue`,
`LensModel`, `GPSLatitude` and the rest of the primary image, Exif and GPS
fields. A name the reader does not know is an error rather than an empty
string, and camera makers' private fields are not decoded. The `<Width>`,
`<Height>`, `<Depth>` and `<JpgComment>` tags read the image's own header
rather than a camera's Exif block, so they answer for a PNG or a BMP where
every `<Exif-…>` returns nothing — BMP, GIF, JPEG, PNG, TIFF and TGA.

A **folder** answers with the first image inside it.

## Misc

| Tag | Gives |
|---|---|
| `<%1>` … `<%9>` | Parts of the filename, split by Setup Parts |
| `<Ask>` / `<Ask-1>` | Ask for user input, once per run |
| `<Clipboard>` | Clipboard contents |
| `<Crc32>` | File CRC32 checksum, eight hex digits |
| `<DetectedExt>` | Extension detected by reading inside the file |
| `<HtmlTitle>` | The `<title>` of an HTML page; nothing for a folder |
| `<FileMax-64>` | Cut the text this field produces to # characters |
| `<PathMax-260>` | Cut this field's text so the folder, a separator and the text fit in # characters |
| `<\>` | Move the file into a subfolder |

`<FileMax>` and `<PathMax>` count only what the field they are in produces
(P25). An extension a name-scoped step keeps is not counted, and neither is the
rest of the name around a partial field such as Add's insert. The tightest
limit wins if a field has several.

`<Crc32>` and `<DetectedExt>` read a regular file's bytes, and answer nothing
for a folder, a named pipe or a device. What they read is cached until the
listing is refreshed.

### `<%n>` and Setup Parts

Setup Parts says what shape your filenames already have, by naming the
separators. Given files called `Metallica - Nothing Else Matters.mp3`, a parts
pattern of `<%1> - <%2>` loads `<%1>` with the artist and `<%2>` with the
title.

Set Date & Time's *Get from filename* source reads a date out of the same
parts: `<%4>` is the year, `<%5>` the month, `<%6>` the day and `<%7>`–`<%9>`
the hour, minute and second. Only the year is required — a missing month or
day is 1, a missing time is midnight — and a two-digit year reads 80–99 as
19xx and 00–79 as 20xx. A file whose name gives no year is left alone. For
`My File 2000-12-31.txt`, the parts pattern `<%1> <%2> <%4>-<%5>-<%6>` sets the
date to 31 December 2000.

### `<\>` moves a file into a subfolder

`<Date-yyyy><\><Name>` sorts photos into one folder per year, creating the
folder if needed. Undo puts the files back and removes the folders it created —
but never one you have since put something else into.

A produced name can go **down** only. It can never go up or out of the folder
it started in.
