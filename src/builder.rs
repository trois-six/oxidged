//! Reading a dataset with options: a size limit, strict mode, a given
//! encoding, a reader or a GEDZIP archive.
//!
//! ```rust
//! use oxidged::GedcomBuilder;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";
//! let data = GedcomBuilder::new().max_file_size(50_000_000).build_from_str(text)?;
//! let ann = data.individuals().first().unwrap();
//! assert_eq!(ann.full_name().as_deref(), Some("Ann Example"));
//! # Ok(())
//! # }
//! ```

use std::io::Read;

use crate::GedcomError;
use crate::encoding::{GedcomEncoding, decode_as, decode_owned, decode_owned_with};
use crate::model::Dataset;
use crate::profile::{Detection, HeadFields, InterpretReport, Interpretation, Profile};
use crate::spec::{Deviation, validate_bytes, validate_text};

/// Reads datasets, with options.
///
/// By default reading is lenient and has no limit: any input is read, its
/// data kept, and only I/O can fail. Options:
///
/// - [`max_file_size`](Self::max_file_size) refuses an input larger than a
///   limit before reading it;
/// - [`strict`](Self::strict) refuses an input that does not follow the
///   specification of the version it declares, with every deviation
///   ([`crate::spec::validate_bytes`]); what strict mode accepts reads
///   exactly as leniently;
/// - [`interpretation`](Self::interpretation) and
///   [`profile`](Self::profile) say how far the producer's conventions are
///   interpreted ([`crate::profile`]): by default the producer is detected
///   and its conventions interpreted without losing anything
///   ([`Interpretation::Lossless`]);
/// - the `…_with_report` methods return, with the dataset, every deviation
///   from the specification and what was interpreted ([`ReadReport`]); the
///   others say nothing.
///
/// A builder is cheap to clone and reusable.
#[derive(Clone, Debug, Default)]
pub struct GedcomBuilder {
    strict: bool,
    max_file_size: Option<usize>,
    interpretation: Interpretation,
    profile: Option<&'static Profile>,
}

/// What reading found besides the dataset: see
/// [`GedcomBuilder::build_from_bytes_with_report`]. A query, not a
/// warnings channel.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadReport {
    /// The producer detected (or chosen), with its evidence.
    pub detection: Detection,
    /// The encoding the bytes were decoded with (`None` for text).
    pub encoding: Option<GedcomEncoding>,
    /// The `HEAD.CHAR` payload as written.
    pub declared_charset: Option<String>,
    /// Every way the input does not follow the specification of the version
    /// it declares, in line order (what strict mode refuses).
    pub deviations: Vec<Deviation>,
    /// What interpretation did.
    pub interpretation: InterpretReport,
}

impl GedcomBuilder {
    /// A builder that reads leniently, without limit.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether to refuse an input that does not follow its specification
    /// (default: no).
    ///
    /// In strict mode, reading runs the validator of [`crate::spec`] on the
    /// input — its encoding, its lines, its structures, its pointers — and
    /// fails with [`GedcomError::NonConformant`] and every deviation found.
    ///
    /// ```rust
    /// use oxidged::{GedcomBuilder, GedcomError};
    ///
    /// let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n";
    /// assert!(GedcomBuilder::new().build_from_str(text).is_ok());
    /// let Err(GedcomError::NonConformant(deviations)) =
    ///     GedcomBuilder::new().strict(true).build_from_str(text)
    /// else {
    ///     panic!()
    /// };
    /// assert_eq!(deviations[0].to_string(), "line 5: SEX \"male\": not a value of enumset-SEX");
    /// ```
    #[must_use]
    pub fn strict(mut self, enabled: bool) -> Self {
        self.strict = enabled;
        self
    }

    /// Refuses an input larger than `bytes` with
    /// [`GedcomError::FileTooLarge`], before reading it (default: no
    /// limit). For a GEDZIP archive, the limit applies to its `gedcom.ged`.
    #[must_use]
    pub fn max_file_size(mut self, bytes: usize) -> Self {
        self.max_file_size = Some(bytes);
        self
    }

    /// How far the producer's conventions are interpreted (default:
    /// [`Interpretation::Lossless`]; [`Interpretation::None`] reads the
    /// file as it is written, at no cost).
    #[must_use]
    pub fn interpretation(mut self, level: Interpretation) -> Self {
        self.interpretation = level;
        self
    }

    /// Reads every file as written by `profile`'s producer, instead of
    /// detecting it (`None`, the default, detects).
    #[must_use]
    pub fn profile(mut self, profile: Option<&'static Profile>) -> Self {
        self.profile = profile;
        self
    }

    /// The interpretation level.
    #[must_use]
    pub fn interpretation_level(&self) -> Interpretation {
        self.interpretation
    }

    /// Whether strict mode is on.
    #[must_use]
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// The size limit, if any.
    #[must_use]
    pub fn file_size_limit(&self) -> Option<usize> {
        self.max_file_size
    }

    fn check_size(&self, size: usize) -> Result<(), GedcomError> {
        match self.max_file_size {
            Some(max) if size > max => Err(GedcomError::FileTooLarge { size, max }),
            _ => Ok(()),
        }
    }

    fn check(&self, deviations: impl FnOnce() -> Vec<Deviation>) -> Result<(), GedcomError> {
        if !self.strict {
            return Ok(());
        }
        let deviations = deviations();
        if deviations.is_empty() {
            Ok(())
        } else {
            Err(GedcomError::NonConformant(deviations.into()))
        }
    }

    /// Reads decoded text. A `String` is kept as the dataset's store,
    /// without a copy (unless interpretation rewrites it).
    ///
    /// # Errors
    ///
    /// [`GedcomError::FileTooLarge`] over the limit; in strict mode,
    /// [`GedcomError::NonConformant`].
    pub fn build_from_str(&self, text: impl Into<String>) -> Result<Dataset, GedcomError> {
        self.read_text(text.into(), None, false)
            .map(|(data, _)| data)
    }

    /// [`build_from_str`](Self::build_from_str), with what reading found
    /// ([`ReadReport`]).
    ///
    /// # Errors
    ///
    /// As [`build_from_str`](Self::build_from_str).
    pub fn build_from_str_with_report(
        &self,
        text: impl Into<String>,
    ) -> Result<(Dataset, ReadReport), GedcomError> {
        self.read_text(text.into(), None, true)
    }

    /// Reads bytes, decoded by the evidence they hold and their `HEAD.CHAR`
    /// (see [`crate::encoding`]) as their producer means it (see
    /// [`crate::profile`]); never fails to decode. A `Vec<u8>` of UTF-8
    /// becomes the dataset's store without a copy.
    ///
    /// # Errors
    ///
    /// [`GedcomError::FileTooLarge`] over the limit; in strict mode,
    /// [`GedcomError::NonConformant`] (an encoding that contradicts the
    /// declared one included).
    ///
    /// ```rust
    /// use oxidged::GedcomBuilder;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let data = GedcomBuilder::new()
    ///     .build_from_bytes(b"0 HEAD\r1 CHAR ANSI\r0 @I1@ INDI\r1 NAME Ren\xE9e /Example/\r0 TRLR\r")?;
    /// let renee = data.individuals().first().unwrap();
    /// assert_eq!(renee.full_name().as_deref(), Some("Renée Example"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn build_from_bytes(&self, bytes: impl Into<Vec<u8>>) -> Result<Dataset, GedcomError> {
        self.read_bytes(bytes.into(), false).map(|(data, _)| data)
    }

    /// [`build_from_bytes`](Self::build_from_bytes), with what reading found:
    /// the producer, the encoding, every deviation from the specification
    /// (as strict mode would list them, without refusing anything) and what
    /// was interpreted.
    ///
    /// ```rust
    /// use oxidged::GedcomBuilder;
    ///
    /// # fn main() -> Result<(), oxidged::GedcomError> {
    /// let (data, report) = GedcomBuilder::new().build_from_bytes_with_report(
    ///     b"0 HEAD\n1 SOUR FTM\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n1 _FUN\n2 DATE 1900\n0 TRLR\n",
    /// )?;
    /// assert_eq!(report.detection.profile().id(), "ftm");
    /// assert_eq!(report.deviations.len(), 1); // SEX male
    /// assert_eq!(report.interpretation.rows(), ["VD-093", "X-020"]); // _FUN
    /// assert_eq!(data.individuals().first().unwrap().events().len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`build_from_bytes`](Self::build_from_bytes).
    pub fn build_from_bytes_with_report(
        &self,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(Dataset, ReadReport), GedcomError> {
        self.read_bytes(bytes.into(), true)
    }

    /// Reads bytes in a given encoding, whatever they declare (see
    /// [`crate::encoding::decode_as`]).
    ///
    /// # Errors
    ///
    /// As [`build_from_str`](Self::build_from_str).
    pub fn build_from_bytes_with_encoding(
        &self,
        bytes: &[u8],
        encoding: GedcomEncoding,
    ) -> Result<Dataset, GedcomError> {
        self.check_size(bytes.len())?;
        let detection = self.detection(bytes);
        let text = decode_as(bytes, encoding);
        self.read_text(text, Some(detection), false)
            .map(|(data, _)| data)
    }

    /// The producer of `bytes`: the chosen profile, else detected from the
    /// header.
    fn detection(&self, bytes: &[u8]) -> Detection {
        match self.profile {
            Some(p) => Detection::chosen(p, bytes),
            None => Detection::from_head(bytes),
        }
    }

    fn read_bytes(
        &self,
        bytes: Vec<u8>,
        report: bool,
    ) -> Result<(Dataset, ReadReport), GedcomError> {
        self.check_size(bytes.len())?;
        let deviations = if report || self.strict {
            validate_bytes(&bytes)
        } else {
            Vec::new()
        };
        self.check(|| deviations.clone())?;
        let interpret = self.interpretation != Interpretation::None;
        if !interpret && !report {
            let data = Dataset::parse(decode_owned(bytes).text);
            return Ok((data, Self::empty_report()));
        }
        let mut detection = self.detection(&bytes);
        let decoded = if interpret {
            decode_owned_with(bytes, &|declared, body| {
                crate::profile::charset_for(&detection, declared, body)
            })
        } else {
            decode_owned(bytes)
        };
        let (encoding, declared_charset) = (decoded.encoding, decoded.declared.clone());
        let (data, interpretation) = crate::profile::read_interpreted(
            decoded.text,
            &mut detection,
            self.interpretation,
            report,
        );
        Ok((
            data,
            ReadReport {
                detection,
                encoding: Some(encoding),
                declared_charset,
                deviations,
                interpretation,
            },
        ))
    }

    fn read_text(
        &self,
        text: String,
        detection: Option<Detection>,
        report: bool,
    ) -> Result<(Dataset, ReadReport), GedcomError> {
        self.check_size(text.len())?;
        let deviations = if report || self.strict {
            validate_text(&text)
        } else {
            Vec::new()
        };
        self.check(|| deviations.clone())?;
        if self.interpretation == Interpretation::None && !report {
            return Ok((Dataset::parse(text), Self::empty_report()));
        }
        let mut detection = detection.unwrap_or_else(|| match self.profile {
            Some(p) => Detection::chosen(p, text.as_bytes()),
            None => Detection::of_head(HeadFields::from_text(&text)),
        });
        let declared_charset = detection.head().charset.clone();
        let (data, interpretation) =
            crate::profile::read_interpreted(text, &mut detection, self.interpretation, report);
        Ok((
            data,
            ReadReport {
                detection,
                encoding: None,
                declared_charset,
                deviations,
                interpretation,
            },
        ))
    }

    fn empty_report() -> ReadReport {
        ReadReport {
            detection: Detection::of_head(HeadFields::default()),
            encoding: None,
            declared_charset: None,
            deviations: Vec::new(),
            interpretation: InterpretReport::default(),
        }
    }

    /// Reads everything `reader` gives, as
    /// [`build_from_bytes`](Self::build_from_bytes) reads bytes. With a
    /// size limit, no more than the limit is read.
    ///
    /// # Errors
    ///
    /// [`GedcomError::Io`] when `reader` fails, otherwise as
    /// [`build_from_bytes`](Self::build_from_bytes).
    pub fn build_from_reader(&self, reader: impl Read) -> Result<Dataset, GedcomError> {
        self.build_from_bytes(self.read_all(reader)?)
    }

    /// [`build_from_reader`](Self::build_from_reader), with what reading
    /// found ([`ReadReport`]).
    ///
    /// # Errors
    ///
    /// As [`build_from_reader`](Self::build_from_reader).
    pub fn build_from_reader_with_report(
        &self,
        reader: impl Read,
    ) -> Result<(Dataset, ReadReport), GedcomError> {
        self.build_from_bytes_with_report(self.read_all(reader)?)
    }

    fn read_all(&self, reader: impl Read) -> Result<Vec<u8>, GedcomError> {
        let mut bytes = Vec::new();
        // Past the limit by one byte at most: enough to refuse the input.
        let limit = self.max_file_size.map_or(u64::MAX, |max| {
            u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1)
        });
        reader.take(limit).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// Reads the dataset of a GEDZIP archive (its `gedcom.ged`), as
    /// [`build_from_bytes`](Self::build_from_bytes) reads bytes.
    ///
    /// ```rust,no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxidged::GedcomBuilder;
    ///
    /// let data = GedcomBuilder::new().build_from_gedzip(std::fs::File::open("family.gdz")?)?;
    /// println!("{} individuals", data.individuals().len());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`GedcomError::Gedzip`] when the archive cannot be read or has no
    /// `gedcom.ged`; otherwise as [`build_from_bytes`](Self::build_from_bytes).
    #[cfg(feature = "gedzip")]
    pub fn build_from_gedzip<R: Read + std::io::Seek>(
        &self,
        archive: R,
    ) -> Result<Dataset, GedcomError> {
        crate::gedzip::GedzipReader::new(archive)?.read_dataset(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n";

    #[test]
    fn defaults() {
        let builder = GedcomBuilder::new();
        assert!(!builder.is_strict());
        assert_eq!(builder.file_size_limit(), None);
        let data = builder.build_from_str(TEXT).unwrap();
        assert_eq!(data.individuals().len(), 1);
        assert_eq!(builder.build_from_bytes(TEXT.as_bytes()).unwrap(), data);
        assert_eq!(builder.build_from_reader(TEXT.as_bytes()).unwrap(), data);
        assert_eq!(
            builder
                .build_from_bytes_with_encoding(TEXT.as_bytes(), GedcomEncoding::Utf8)
                .unwrap(),
            data
        );
    }

    #[test]
    fn size_limit() {
        let builder = GedcomBuilder::new().max_file_size(10);
        for result in [
            builder.build_from_str(TEXT),
            builder.build_from_bytes(TEXT.as_bytes()),
            builder.build_from_reader(TEXT.as_bytes()),
            builder.build_from_bytes_with_encoding(TEXT.as_bytes(), GedcomEncoding::Utf8),
        ] {
            assert!(matches!(
                result,
                Err(GedcomError::FileTooLarge { max: 10, .. })
            ));
        }
        // The reader stops past the limit.
        assert!(matches!(
            builder.build_from_reader(std::io::repeat(b'0')),
            Err(GedcomError::FileTooLarge { size: 11, max: 10 })
        ));
        assert!(
            GedcomBuilder::new()
                .max_file_size(TEXT.len())
                .build_from_str(TEXT)
                .is_ok()
        );
    }

    #[test]
    fn strict_mode_lists_every_deviation() {
        let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n1 FAMC @F9@\n0 TRLR\n";
        let strict = GedcomBuilder::new().strict(true);
        let Err(GedcomError::NonConformant(deviations)) = strict.build_from_str(text) else {
            panic!()
        };
        assert_eq!(deviations.len(), 2, "{deviations:?}");
        assert!(strict.build_from_str(TEXT.replace("5.5.1", "7.0")).is_ok());
        // The encoding counts too: a 7.x file is UTF-8.
        let latin1 = b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ren\xE9e\n0 TRLR\n";
        assert!(GedcomBuilder::new().build_from_bytes(latin1).is_ok());
        assert!(matches!(
            strict.build_from_bytes(latin1),
            Err(GedcomError::NonConformant(_))
        ));
    }
}
