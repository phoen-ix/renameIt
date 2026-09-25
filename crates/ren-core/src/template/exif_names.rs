//! The names `<Exif-…>` accepts.
//!
//! The Exif reader keys every field by the name `kamadak-exif` gives its tag,
//! which is a closed set: a tag the crate does not know comes back as
//! `Tag(Tiff, 1234)`, and maker-note fields are not decoded at all. So a name
//! outside the set can never resolve, and accepting it at compile time meant a
//! typo such as `<Exif-Mkae>` rendered as an empty string in every filename of
//! the batch — exactly what D29 exists to prevent, and what D57 already
//! refused for the ID3 family.
//!
//! The crate has no public list of its names, but its tag constants are
//! public and each one displays as its name. Listing the constants rather than
//! the strings makes a misspelt entry a compile error, and the test in
//! `tests/tags.rs` that parses every field the reader returns from the
//! fixtures catches a crate upgrade that renames one. The three IFD pointers
//! are left out: the reader never returns them as fields.

use std::sync::OnceLock;

use exif::Tag;

/// Every tag `kamadak-exif` 0.6 names, except the IFD pointers.
const KNOWN: [Tag; 150] = [
    // The primary image (IFD0).
    Tag::ImageWidth,
    Tag::ImageLength,
    Tag::BitsPerSample,
    Tag::Compression,
    Tag::PhotometricInterpretation,
    Tag::ImageDescription,
    Tag::Make,
    Tag::Model,
    Tag::StripOffsets,
    Tag::Orientation,
    Tag::SamplesPerPixel,
    Tag::RowsPerStrip,
    Tag::StripByteCounts,
    Tag::XResolution,
    Tag::YResolution,
    Tag::PlanarConfiguration,
    Tag::ResolutionUnit,
    Tag::TransferFunction,
    Tag::Software,
    Tag::DateTime,
    Tag::Artist,
    Tag::WhitePoint,
    Tag::PrimaryChromaticities,
    Tag::TileOffsets,
    Tag::TileByteCounts,
    Tag::JPEGInterchangeFormat,
    Tag::JPEGInterchangeFormatLength,
    Tag::YCbCrCoefficients,
    Tag::YCbCrSubSampling,
    Tag::YCbCrPositioning,
    Tag::ReferenceBlackWhite,
    Tag::Copyright,
    // The Exif sub-IFD: exposure, lens, capture times.
    Tag::ExposureTime,
    Tag::FNumber,
    Tag::ExposureProgram,
    Tag::SpectralSensitivity,
    Tag::PhotographicSensitivity,
    Tag::OECF,
    Tag::SensitivityType,
    Tag::StandardOutputSensitivity,
    Tag::RecommendedExposureIndex,
    Tag::ISOSpeed,
    Tag::ISOSpeedLatitudeyyy,
    Tag::ISOSpeedLatitudezzz,
    Tag::ExifVersion,
    Tag::DateTimeOriginal,
    Tag::DateTimeDigitized,
    Tag::OffsetTime,
    Tag::OffsetTimeOriginal,
    Tag::OffsetTimeDigitized,
    Tag::ComponentsConfiguration,
    Tag::CompressedBitsPerPixel,
    Tag::ShutterSpeedValue,
    Tag::ApertureValue,
    Tag::BrightnessValue,
    Tag::ExposureBiasValue,
    Tag::MaxApertureValue,
    Tag::SubjectDistance,
    Tag::MeteringMode,
    Tag::LightSource,
    Tag::Flash,
    Tag::FocalLength,
    Tag::SubjectArea,
    Tag::MakerNote,
    Tag::UserComment,
    Tag::SubSecTime,
    Tag::SubSecTimeOriginal,
    Tag::SubSecTimeDigitized,
    Tag::Temperature,
    Tag::Humidity,
    Tag::Pressure,
    Tag::WaterDepth,
    Tag::Acceleration,
    Tag::CameraElevationAngle,
    Tag::FlashpixVersion,
    Tag::ColorSpace,
    Tag::PixelXDimension,
    Tag::PixelYDimension,
    Tag::RelatedSoundFile,
    Tag::FlashEnergy,
    Tag::SpatialFrequencyResponse,
    Tag::FocalPlaneXResolution,
    Tag::FocalPlaneYResolution,
    Tag::FocalPlaneResolutionUnit,
    Tag::SubjectLocation,
    Tag::ExposureIndex,
    Tag::SensingMethod,
    Tag::FileSource,
    Tag::SceneType,
    Tag::CFAPattern,
    Tag::CustomRendered,
    Tag::ExposureMode,
    Tag::WhiteBalance,
    Tag::DigitalZoomRatio,
    Tag::FocalLengthIn35mmFilm,
    Tag::SceneCaptureType,
    Tag::GainControl,
    Tag::Contrast,
    Tag::Saturation,
    Tag::Sharpness,
    Tag::DeviceSettingDescription,
    Tag::SubjectDistanceRange,
    Tag::ImageUniqueID,
    Tag::CameraOwnerName,
    Tag::BodySerialNumber,
    Tag::LensSpecification,
    Tag::LensMake,
    Tag::LensModel,
    Tag::LensSerialNumber,
    Tag::CompositeImage,
    Tag::SourceImageNumberOfCompositeImage,
    Tag::SourceExposureTimesOfCompositeImage,
    Tag::Gamma,
    // The GPS sub-IFD.
    Tag::GPSVersionID,
    Tag::GPSLatitudeRef,
    Tag::GPSLatitude,
    Tag::GPSLongitudeRef,
    Tag::GPSLongitude,
    Tag::GPSAltitudeRef,
    Tag::GPSAltitude,
    Tag::GPSTimeStamp,
    Tag::GPSSatellites,
    Tag::GPSStatus,
    Tag::GPSMeasureMode,
    Tag::GPSDOP,
    Tag::GPSSpeedRef,
    Tag::GPSSpeed,
    Tag::GPSTrackRef,
    Tag::GPSTrack,
    Tag::GPSImgDirectionRef,
    Tag::GPSImgDirection,
    Tag::GPSMapDatum,
    Tag::GPSDestLatitudeRef,
    Tag::GPSDestLatitude,
    Tag::GPSDestLongitudeRef,
    Tag::GPSDestLongitude,
    Tag::GPSDestBearingRef,
    Tag::GPSDestBearing,
    Tag::GPSDestDistanceRef,
    Tag::GPSDestDistance,
    Tag::GPSProcessingMethod,
    Tag::GPSAreaInformation,
    Tag::GPSDateStamp,
    Tag::GPSDifferential,
    Tag::GPSHPositioningError,
    // The interoperability sub-IFD.
    Tag::InteroperabilityIndex,
    Tag::InteroperabilityVersion,
    Tag::RelatedImageFileFormat,
    Tag::RelatedImageWidth,
    Tag::RelatedImageLength,
];

/// Whether `name` is a field the Exif reader can return, compared without
/// regard to case — the reader's own lookup is case-insensitive too.
pub(crate) fn is_known(name: &str) -> bool {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES
        .get_or_init(|| KNOWN.iter().map(Tag::to_string).collect())
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each constant displays as a name, never as the `Tag(ctx, n)` fallback
    /// the crate prints for a tag it does not know.
    #[test]
    fn every_listed_tag_has_a_name() {
        for tag in KNOWN {
            let name = tag.to_string();
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric()),
                "{name:?} is not a field name"
            );
        }
    }

    #[test]
    fn names_match_without_regard_to_case() {
        assert!(is_known("Make"));
        assert!(is_known("make"));
        assert!(is_known("GPSLatitude"));
        assert!(is_known("DateTimeOriginal"));
        assert!(!is_known("Mkae"));
        assert!(!is_known("ExifIFDPointer"));
        assert!(!is_known(""));
    }
}
