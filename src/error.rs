//! The error of reading a dataset.

use std::fmt;

use crate::spec::Deviation;

/// Why reading a GEDCOM dataset failed.
///
/// Reading itself never fails: bytes of any encoding are decoded, and every
/// line is read by fixed, lenient rules that keep its data. Only the input
/// (I/O), a limit the caller set, strict mode and a GEDZIP container can
/// make reading fail.
#[non_exhaustive]
#[derive(Debug)]
pub enum GedcomError {
    /// Reading the input failed.
    Io(std::io::Error),
    /// The input is larger than the limit set with
    /// [`GedcomBuilder::max_file_size`](crate::GedcomBuilder::max_file_size).
    FileTooLarge {
        /// The size of the input, in bytes.
        size: usize,
        /// The limit, in bytes.
        max: usize,
    },
    /// In strict mode
    /// ([`GedcomBuilder::strict`](crate::GedcomBuilder::strict)), the input
    /// does not follow the specification of the version it declares: every
    /// deviation found, in line order.
    NonConformant(Box<[Deviation]>),
    /// The GEDZIP archive cannot be read.
    #[cfg(feature = "gedzip")]
    Gedzip(crate::gedzip::GedzipError),
}

impl fmt::Display for GedcomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GedcomError::Io(e) => write!(f, "I/O error: {e}"),
            GedcomError::FileTooLarge { size, max } => {
                write!(f, "the input of {size} bytes exceeds the {max}-byte limit")
            }
            GedcomError::NonConformant(deviations) => match deviations.as_ref() {
                [] => f.write_str("the input does not conform to its specification"),
                [only] => write!(f, "the input does not conform to its specification: {only}"),
                [first, rest @ ..] => write!(
                    f,
                    "the input does not conform to its specification: {first} (and {} more)",
                    rest.len()
                ),
            },
            #[cfg(feature = "gedzip")]
            GedcomError::Gedzip(e) => write!(f, "GEDZIP error: {e}"),
        }
    }
}

impl std::error::Error for GedcomError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GedcomError::Io(e) => Some(e),
            #[cfg(feature = "gedzip")]
            GedcomError::Gedzip(e) => Some(e),
            GedcomError::FileTooLarge { .. } | GedcomError::NonConformant(_) => None,
        }
    }
}

impl From<std::io::Error> for GedcomError {
    fn from(err: std::io::Error) -> Self {
        GedcomError::Io(err)
    }
}

#[cfg(feature = "gedzip")]
impl From<crate::gedzip::GedzipError> for GedcomError {
    fn from(err: crate::gedzip::GedzipError) -> Self {
        GedcomError::Gedzip(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn display_and_source() {
        let io = GedcomError::from(std::io::Error::other("disk"));
        assert_eq!(io.to_string(), "I/O error: disk");
        assert!(io.source().is_some());

        let size = GedcomError::FileTooLarge { size: 10, max: 5 };
        assert_eq!(
            size.to_string(),
            "the input of 10 bytes exceeds the 5-byte limit"
        );
        assert!(size.source().is_none());

        let deviations = crate::spec::validate_text("0 HEAD\n1 GEDC\n2  VERS 7.0\n0 TRLR\n");
        let strict = GedcomError::NonConformant(deviations.into());
        assert!(
            strict
                .to_string()
                .starts_with("the input does not conform to its specification: line 3")
        );
    }
}
