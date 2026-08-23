# Tag reference

Any field that builds text takes `<tags>`. A tag is everything between a `<`
and the next `>`; everything else is literal text.

**A tag you mistype is an error, not an empty string** (D29). The editor names
it while you are typing, rather than letting one silent typo run across a
thousand files.

Tag names are matched case-insensitively and render back in the spelling the
tag menu shows. Where a tag takes a number it is written after a hyphen —
`<Left-3>`, `<Mid-2-5>`, `<Rnd3-1-100>`.

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
| `<FLetter>` | First letter in the name |
| `<FLetterN1>` | First letter, numbers included |
| `<FLetterN2>` | First letter, numbers included and substituted |

## Size

| Tag | Gives |
|---|---|
| `<Size>` | File size, scaled unit |
| `<SizeB>` | File size in bytes |
| `<SizeB-8>` | File size in bytes, padded with # zeros |

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
| `<SDirSize>` | Size through subfolders, scaled unit |
| `<FirstFileInFolder>` | Name of the first file in the folder |
| `<FirstFileInFolder-Ext>` | The same, with its extension |
| `<HtmlTitle>` | The `<title>` of an HTML page |

## Date and time

| Tag | Gives |
|---|---|
| `<Date>` / `<Time>` | Modified date / time |
| `<CDate>` / `<CTime>` | Created date / time |
| `<ADate>` / `<ATime>` | Accessed date / time |
| `<Date-yyyy-mm-dd>` | Custom formatted date |
| `<NowDate>` / `<NowTime>` | Date / time at the start of the run |

The default formats are `yyyy-mm-dd` and `Hh.Mm.Ss` — ISO 8601 order, so a
listing sorts correctly by name.

Two format codes are worth knowing: **`m` after `h` or `Hh` means *minute*, not
month**, and `N`/`Nn` are also minutes and are matched case-sensitively so that
ordinary words survive. `\` escapes the next character.

## Numbers

| Tag | Gives |
|---|---|
| `<Counter>` | The run counter (start, step, padding set in Counter Setup) |
| `<NumFiles>` | Number of items in the run |
| `<Rnd>` | Random character, A–Z |
| `<Rnd3>` | Random number, 0–9, # digits |
| `<Rnd3-1-100>` | Random number in a custom range |
| `<RndZ3-1-100>` | The same, zero-padded |

Randomness is seeded per run (P16), so a preview and the rename that follows it
produce the same numbers.

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
| `<ExifDate-yyyy-mm-dd>` | Exif date, custom format |
| `<Exif-Make>`, `<Exif-Model>` | Camera manufacturer and model |
| `<Exif-PixelXDimension>`, `<Exif-PixelYDimension>` | Image width and height |
| `<Exif-FNumber>`, `<Exif-ExposureTime>` | Aperture, shutter speed |
| `<Exif-PhotographicSensitivity>` | ISO speed |
| `<Exif-FocalLength>`, `<Exif-Orientation>` | Focal length, orientation |
| `<Width>`, `<Height>` | Pixel dimensions from the image header |
| `<Depth>`, `<Depthb>` | Colour depth, in colours / in bits |
| `<JpgComment>` | The comment stored inside a JPEG |

`<Exif-*>` accepts any field name the Exif reader knows. The `<Width>`,
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
| `<Crc32>` | File CRC32 checksum |
| `<DetectedExt>` | Extension detected by reading inside the file |
| `<FileMax-64>` | Limit the filename to # characters |
| `<PathMax-260>` | Limit the whole path to # characters |
| `<\>` | Move the file into a subfolder |

### `<%n>` and Setup Parts

Setup Parts says what shape your filenames already have, by naming the
separators. Given files called `Metallica - Nothing Else Matters.mp3`, a parts
pattern of `<%1> - <%2>` loads `<%1>` with the artist and `<%2>` with the
title.

### `<\>` moves a file into a subfolder

`<Date-yyyy><\><Name>` sorts photos into one folder per year, creating the
folder if needed. Undo puts the files back and removes the folders it created —
but never one you have since put something else into.

A produced name can go **down** only. It can never go up or out of the folder
it started in.
