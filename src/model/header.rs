//! The header of a dataset (`HEAD`) and its substructures.

use super::address::Address;
use super::date::ExactDate;
use std::borrow::Cow;

use super::driver::{Slot, StdTag, WriteCx, gedcom_struct};
use super::enums::{CharacterSet, GedcomForm};
use super::note::{Note, TextTranslation};
use super::text::{Store, Text, XrefId};
use crate::tree::{Flat, FlatPayload, UNKNOWN};
use crate::version::VersionRules;
use crate::writer::head::PRODUCT;

gedcom_struct! {
    /// The header (`HEAD`): what the file is (`GEDC`), its character set
    /// (5.5.1 `CHAR`), the product that wrote it (`SOUR`), its recipient,
    /// date, submitter, copyright, language and place form, a note, the
    /// extension tags it uses (7.x `SCHMA`), and 7.1's title and
    /// description.
    ///
    /// The writer completes it for the target version: it writes `GEDC`
    /// (and in 5.5.1 `CHAR`, `SOUR` and `SUBM`) as that version requires,
    /// whatever the header holds.
    pub struct Header => HeaderRow {
        /// The GEDCOM version and form (`GEDC`).
        "GEDC" => gedcom: Option<GedcomInfo>,
        /// The character set of a 5.5.1 file (`CHAR`).
        "CHAR" => charset: Option<Charset>,
        /// The product that wrote the file (`SOUR`).
        "SOUR" => source: Option<HeaderSource>,
        /// The product the file is for (`DEST`).
        "DEST" => destination: Option<Text>,
        /// When the file was written (`DATE`).
        "DATE" => date: Option<ExactDate>,
        /// The submitter of the file (`SUBM`).
        "SUBM" => submitter: Option<XrefId>,
        /// The submission record (5.5.1 `SUBN`).
        "SUBN" => submission: Option<XrefId>,
        /// The file's name (5.5.1 `FILE`).
        "FILE" => file: Option<Text>,
        /// The copyright of the file (`COPR`).
        "COPR" => copyright: Option<Text>,
        /// The language of the file's texts (`LANG`).
        "LANG" => language: Option<Text>,
        /// The jurisdictions places name by default (`PLAC`).
        "PLAC" => place: Option<HeaderPlace>,
        /// Notes about the file (`NOTE`, 7.x `SNOTE`; one before 7.1).
        "NOTE" | "SNOTE" => notes: Vec<Note>,
        /// The extension tags the file uses and their URIs (7.x `SCHMA`).
        "SCHMA" => schema: Option<Schema>,
        /// The title of the file (7.1 `TITL`).
        "TITL" => title: Option<HeaderText>,
        /// A description of the file (7.1 `DESC`).
        "DESC" => description: Option<HeaderText>,
    }
    spec {
        v551: ["HEADER.HEAD"],
        v70: ["HEAD"],
        v71: ["HEAD"],
    }
}

gedcom_struct! {
    /// What a file is (`GEDC`): the GEDCOM version (`VERS`) and, in 5.5.1,
    /// its form (`FORM`), any value of which is kept.
    pub struct GedcomInfo => GedcomInfoRow {
        /// The version (`VERS`).
        "VERS" => version: Option<Text>,
        /// The form (5.5.1 `FORM`).
        "FORM" => form: Option<GedcomForm>,
    }
    spec {
        v551: ["HEADER.HEAD.GEDC"],
        v70: ["GEDC"],
        v71: ["GEDC"],
    }
}

impl Header {
    /// The version the header declares (`GEDC.VERS`), as written.
    #[must_use]
    pub fn declared_version<'a, S: AsRef<Store> + ?Sized>(
        &'a self,
        store: &'a S,
    ) -> Option<std::borrow::Cow<'a, str>> {
        let vers = self.gedcom.as_ref()?.version.as_ref()?;
        Some(vers.to_str(store))
    }

    /// Writes the header into `out` as the writer completes it for
    /// `rules` in UTF-8 ([`crate::writer::head`]), its fields in model
    /// order: `GEDC` with the target's version (and 5.5.1 form), 5.5.1
    /// `CHAR UTF-8`, the `SOUR` 5.5.1 requires (`oxidged` when it has
    /// none), the pointer `submitter` (if any) as its `SUBM` when it has none, then its other
    /// fields. `false`, having written nothing, when it holds a structure
    /// the completion would keep where the typed model does not write it
    /// (an unknown or extension substructure, a payload or identifier set
    /// aside).
    pub(crate) fn write_completed_flat<'s>(
        &'s self,
        cx: &WriteCx<'s>,
        out: &mut Flat<'s>,
        rules: &VersionRules,
        submitter: FlatPayload<'s>,
    ) -> bool {
        // The completion replaces `GEDC.VERS` and `GEDC.FORM`, and `CHAR`
        // whole.
        let gedc_kept = self.gedcom.as_ref().is_some_and(|g| {
            g.extra
                .iter()
                .any(|n| !matches!(cx.store.tag(n.tag), "VERS" | "FORM"))
        });
        if !self.extra.is_empty() || gedc_kept {
            return false;
        }
        macro_rules! tag {
            ($t:literal) => {
                const { StdTag::new($t) }
            };
        }
        let text = |s: &'static str| FlatPayload::Text(Cow::Borrowed(s), UNKNOWN);
        let at = out.open_typed("HEAD", tag!("HEAD").index, None, FlatPayload::None, None);
        let gedc = out.open_typed("GEDC", tag!("GEDC").index, None, FlatPayload::None, None);
        out.leaf_typed("VERS", tag!("VERS").index, text(rules.vers_payload), None);
        if let Some(form) = rules.gedc_form {
            out.leaf_typed("FORM", tag!("FORM").index, text(form), None);
        }
        out.close(gedc);
        if rules.head_char {
            out.leaf_typed("CHAR", tag!("CHAR").index, text("UTF-8"), None);
        }
        match &self.source {
            None if rules.head_sour_subm => {
                let sour = out.open_typed("SOUR", tag!("SOUR").index, None, text(PRODUCT), None);
                let version = text(env!("CARGO_PKG_VERSION"));
                out.leaf_typed("VERS", tag!("VERS").index, version, None);
                out.leaf_typed("NAME", tag!("NAME").index, text(PRODUCT), None);
                out.close(sour);
            }
            source => Slot::to_flat(source, tag!("SOUR"), cx, out),
        }
        Slot::to_flat(&self.destination, tag!("DEST"), cx, out);
        Slot::to_flat(&self.date, tag!("DATE"), cx, out);
        match (&self.submitter, submitter) {
            (None, FlatPayload::None) | (Some(_), _) => {
                Slot::to_flat(&self.submitter, tag!("SUBM"), cx, out);
            }
            (None, pointer) => out.leaf_typed("SUBM", tag!("SUBM").index, pointer, None),
        }
        Slot::to_flat(&self.submission, tag!("SUBN"), cx, out);
        Slot::to_flat(&self.file, tag!("FILE"), cx, out);
        Slot::to_flat(&self.copyright, tag!("COPR"), cx, out);
        Slot::to_flat(&self.language, tag!("LANG"), cx, out);
        Slot::to_flat(&self.place, tag!("PLAC"), cx, out);
        Slot::to_flat(&self.notes, tag!("NOTE"), cx, out);
        Slot::to_flat(&self.schema, tag!("SCHMA"), cx, out);
        Slot::to_flat(&self.title, tag!("TITL"), cx, out);
        Slot::to_flat(&self.description, tag!("DESC"), cx, out);
        out.close(at);
        true
    }
}

gedcom_struct! {
    /// The character set of a 5.5.1 file (`CHAR`) and its version.
    pub struct Charset => CharsetRow {
        @payload
        /// The character set.
        value: CharacterSet;
        /// Its version (`VERS`).
        "VERS" => version: Option<Text>,
    }
    spec {
        v551: ["HEADER.HEAD.CHAR"],
        v70: [],
        v71: [],
    }
}

gedcom_struct! {
    /// The product that wrote a file (`SOUR` of `HEAD`): its identifier,
    /// version and name, its maker (`CORP`) and the data it was taken from
    /// (`DATA`).
    pub struct HeaderSource => HeaderSourceRow {
        @payload
        /// The product's identifier.
        product: Text;
        /// Its version (`VERS`).
        "VERS" => version: Option<Text>,
        /// Its name (`NAME`).
        "NAME" => name: Option<Text>,
        /// Its maker (`CORP`).
        "CORP" => corporation: Option<Corporation>,
        /// The data it was taken from (`DATA`).
        "DATA" => data: Option<HeaderSourceData>,
    }
    spec {
        v551: ["HEADER.HEAD.SOUR"],
        v70: ["HEAD-SOUR"],
        v71: ["HEAD-SOUR"],
    }
}

gedcom_struct! {
    /// The maker of a product (`CORP`): its name, address and contacts,
    /// every one kept.
    pub struct Corporation => CorporationRow {
        @payload
        /// The name.
        name: Text;
        /// The address (`ADDR`).
        "ADDR" => address: Option<Address>,
        /// Telephone numbers (`PHON`).
        "PHON" => phones: Vec<Text>,
        /// E-mail addresses (`EMAIL`).
        "EMAIL" => emails: Vec<Text>,
        /// Fax numbers (`FAX`).
        "FAX" => faxes: Vec<Text>,
        /// Web pages (`WWW`).
        "WWW" => websites: Vec<Text>,
    }
    spec {
        v551: ["HEADER.HEAD.SOUR.CORP"],
        v70: ["CORP"],
        v71: ["CORP"],
    }
}

gedcom_struct! {
    /// The data a product took a file from (`DATA` of `HEAD.SOUR`): its
    /// name, date and copyright.
    pub struct HeaderSourceData => HeaderSourceDataRow {
        @payload
        /// The name of the data.
        name: Text;
        /// Its date (`DATE`).
        "DATE" => date: Option<ExactDate>,
        /// Its copyright (`COPR`).
        "COPR" => copyright: Option<Text>,
    }
    spec {
        v551: ["HEADER.HEAD.SOUR.DATA"],
        v70: ["HEAD-SOUR-DATA"],
        v71: ["HEAD-SOUR-DATA"],
    }
}

gedcom_struct! {
    /// The default jurisdictions of a file's places (`PLAC` of `HEAD`).
    pub struct HeaderPlace => HeaderPlaceRow {
        /// The jurisdictions, comma-separated (`FORM`).
        "FORM" => form: Option<Text>,
    }
    spec {
        v551: ["HEADER.HEAD.PLAC"],
        v70: ["HEAD-PLAC"],
        v71: ["HEAD-PLAC"],
    }
}

gedcom_struct! {
    /// The extension tags a file uses (7.x `SCHMA`): each `TAG` is a tag
    /// definition (the `TagDef` data type, 7.0.17), an extension tag, a
    /// space and a URI reference (`_LOC http://…`).
    pub struct Schema => SchemaRow {
        /// The tags and their URIs (`TAG`).
        "TAG" => tags: Vec<Text>,
    }
    spec {
        v551: [],
        v70: ["SCHMA"],
        v71: ["SCHMA"],
    }
}

gedcom_struct! {
    /// A title or description of a file (7.1 `TITL` or `DESC` of `HEAD`),
    /// its language and translations.
    pub struct HeaderText => HeaderTextRow {
        @payload
        /// The text.
        text: Text;
        /// Its language (`LANG`).
        "LANG" => language: Option<Text>,
        /// Translations (`TRAN`).
        "TRAN" => translations: Vec<TextTranslation>,
    }
    spec {
        v551: [],
        v70: [],
        v71: ["HEAD-DESC", "HEAD-TITL"],
    }
}
