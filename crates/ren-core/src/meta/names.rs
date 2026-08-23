//! The `<ID3-*>` tag names, mapped onto lofty's format-agnostic keys.
//!
//! Forty-nine of them. The table buys two things a direct frame-ID lookup
//! would not: the name a user types stays stable whatever lofty calls it
//! underneath, and because `ItemKey` is format-agnostic, every one of these
//! works on a FLAC or an MP4 as well as on an MP3.

use lofty::tag::ItemKey;

/// `(the name a user types, the key lofty knows it by)`.
///
/// Sorted, so a reader can find a name at a glance and a binary search is
/// possible if this ever grows enough to want one. Verified frame by frame
/// against lofty's own ID3v2 table rather than guessed.
pub const ID3: &[(&str, ItemKey)] = &[
    ("AlbumArtist", ItemKey::AlbumArtist),
    ("AlbumSortOrder", ItemKey::AlbumTitleSortOrder),
    ("ArtistURL", ItemKey::TrackArtistUrl),
    ("AudioSourceURL", ItemKey::AudioSourceUrl),
    ("AudioURL", ItemKey::AudioFileUrl),
    // TBPM. `IntegerBpm` is the one lofty maps the frame to; `Bpm` is the
    // floating-point Vorbis/MP4 spelling and would read nothing from an MP3.
    ("BeatsPerMinute", ItemKey::IntegerBpm),
    ("CommercialInfo", ItemKey::CommercialInformationUrl),
    ("Composer", ItemKey::Composer),
    ("Conductor", ItemKey::Conductor),
    ("ContentGroupDesc", ItemKey::ContentGroup),
    ("Copyright", ItemKey::CopyrightMessage),
    ("DiscNumber", ItemKey::DiscNumber),
    ("DiscsTotal", ItemKey::DiscTotal),
    ("EncSoftHard", ItemKey::EncoderSoftware),
    ("EncTime", ItemKey::EncodingTime),
    ("EncodedBy", ItemKey::EncodedBy),
    // the historical misspelling for TOWN, "File owner". Kept exactly: a user who
    // copied it out of the tag list has to get what they copied.
    ("FilOowner", ItemKey::FileOwner),
    ("IRadioName", ItemKey::InternetRadioStationName),
    ("IRadioOwner", ItemKey::InternetRadioStationOwner),
    ("IRadioURL", ItemKey::RadioStationUrl),
    ("ISRC", ItemKey::Isrc),
    ("InitialKey", ItemKey::InitialKey),
    // TPE4, "Interpreted, remixed, or otherwise modified by".
    ("InterpretedBy", ItemKey::Remixer),
    ("Languages", ItemKey::Language),
    ("Lyricist", ItemKey::Lyricist),
    ("Lyrics", ItemKey::Lyrics),
    ("Mediatype", ItemKey::OriginalMediaType),
    ("Mood", ItemKey::Mood),
    // TLEN, a duration in milliseconds. Distinct from `<Length>`, which is
    // measured from the audio rather than read out of a tag.
    ("MsLength", ItemKey::Length),
    ("OriginalAlbum", ItemKey::OriginalAlbumTitle),
    ("OriginalArtist", ItemKey::OriginalArtist),
    ("OriginalFilename", ItemKey::OriginalFileName),
    ("OriginalLyricist", ItemKey::OriginalLyricist),
    ("OriginalReleaseTime", ItemKey::OriginalReleaseDate),
    ("PaymentURL", ItemKey::PaymentUrl),
    ("PerfSortOrder", ItemKey::TrackArtistSortOrder),
    ("Publisher", ItemKey::Publisher),
    ("PublisherURL", ItemKey::PublisherUrl),
    ("RecordingTime", ItemKey::RecordingDate),
    ("ReleaseTime", ItemKey::ReleaseDate),
    ("SetSubtitle", ItemKey::SetSubtitle),
    ("TaggingTime", ItemKey::TaggingTime),
    ("TitleSortOrder", ItemKey::TrackTitleSortOrder),
];

/// The six names lofty has no key for.
///
/// Named rather than silently absent, so `<ID3-FileType>` can say *"not
/// supported"* instead of rendering nothing — D29's rule, kept for a family
/// where the alternative is an empty string in a name.
///
/// All six are ID3v2 frames with no cross-format equivalent, which is exactly
/// why lofty does not model them: `TFLT` (file type), `TIPL` (involved people),
/// `WXXX` (user-defined URL), `TPRO` (produced notice), `TMCL` (musician
/// credits) and `TORY` (original release *year*, superseded by `TDOR`, which
/// `<ID3-OriginalReleaseTime>` already reads).
pub const ID3_UNSUPPORTED: [&str; 6] = [
    "FileType",
    "InvolvedPeople",
    "MusicianCredits",
    "OriginalReleaseYear",
    "ProducedNotice",
    "UserDefinedURL",
];

/// The key for a name, case-insensitively.
///
/// Case-insensitive because every other tag in the engine is (`tag.rs` folds
/// the head before matching), and making this family the exception would mean
/// `<id3-composer>` failing while `<date>` works — a distinction no user can
/// predict. The menu inserts the canonical spelling either way.
pub fn id3_key(name: &str) -> Option<ItemKey> {
    ID3.iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, key)| *key)
}

/// Whether this is a known name we cannot read.
pub fn id3_is_unsupported(name: &str) -> bool {
    ID3_UNSUPPORTED
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forty-nine names exist. We read 43 and name the other six rather than
    /// letting them render as nothing.
    #[test]
    fn every_name_the_manual_lists_is_accounted_for() {
        assert_eq!(ID3.len(), 43);
        assert_eq!(ID3.len() + ID3_UNSUPPORTED.len(), 49);
    }

    #[test]
    fn the_table_is_sorted_and_has_no_duplicates() {
        let names: Vec<&str> = ID3.iter().map(|(n, _)| *n).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "keep it sorted so a name is findable");
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "a name is listed twice");
    }

    /// A supported name and an unsupported one must never be the same name, or
    /// one of the two answers is unreachable.
    #[test]
    fn no_name_is_both_supported_and_not() {
        for (name, _) in ID3 {
            assert!(!id3_is_unsupported(name), "{name} is in both tables");
        }
    }

    #[test]
    fn a_name_is_found_whatever_case_it_is_written_in() {
        assert_eq!(id3_key("AlbumArtist"), Some(ItemKey::AlbumArtist));
        assert_eq!(id3_key("albumartist"), Some(ItemKey::AlbumArtist));
        assert_eq!(id3_key("ALBUMARTIST"), Some(ItemKey::AlbumArtist));
        assert_eq!(id3_key("Nonsense"), None);
    }

    /// the historical misspelling is part of the contract: a user who copied
    /// `<ID3-FilOowner>` out of the tag list has to get what they copied.
    #[test]
    fn the_manuals_typo_is_reproduced_exactly() {
        assert_eq!(id3_key("FilOowner"), Some(ItemKey::FileOwner));
        assert_eq!(
            id3_key("FileOwner"),
            None,
            "the correct spelling is not the one it lists"
        );
    }

    /// TBPM maps to `IntegerBpm`, not `Bpm` — the latter is the floating-point
    /// Vorbis spelling and reads nothing from an MP3.
    #[test]
    fn beats_per_minute_uses_the_key_the_id3_frame_maps_to() {
        assert_eq!(id3_key("BeatsPerMinute"), Some(ItemKey::IntegerBpm));
    }
}
