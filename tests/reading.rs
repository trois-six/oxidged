//! Reading: the input API (text, bytes, readers, streaming, size limit,
//! strict mode), leniency towards malformed input, encodings and line
//! terminators, continuation lines (`CONC`/`CONT`) and the `@@` escape.
//! Every name and value is fictitious.

use std::io::BufReader;

use oxidged::encoding::{DecodeReader, GedcomEncoding, decode, encode};
use oxidged::model::{
    Address, CharacterSet, Dataset, EventKind, Individual, Multimedia, Note, NoteContent,
    RecordRef, Repository, Sex, SharedNote, Source, Text,
};
use oxidged::tree::Structure;
use oxidged::{GedcomBuilder, GedcomError, GedcomStreamParser, GedcomVersion, GedcomWriter};

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn read(bytes: &[u8]) -> Dataset {
    GedcomBuilder::new().build_from_bytes(bytes).unwrap()
}

fn stream(bytes: &[u8]) -> Dataset {
    GedcomStreamParser::new(BufReader::with_capacity(7, bytes))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The first name of the first individual, as written.
fn name(data: &Dataset) -> String {
    data.individuals().get(0).unwrap().into_owned().names[0]
        .value
        .to_str(data)
        .into_owned()
}

/// The text of an inline note.
fn note_text(data: &Dataset, note: &Note) -> String {
    match &note.content {
        NoteContent::Text(text) => text.to_str(data).into_owned(),
        NoteContent::Shared(id) => panic!("shared note {}", data.store().xref(*id)),
    }
}

fn text(data: &Dataset, text: Option<&Text>) -> Option<String> {
    text.map(|t| t.to_str(data).into_owned())
}

fn write(data: &Dataset) -> String {
    GedcomWriter::new().write_to_string(data).unwrap()
}

/// The record of a dataset with an identifier, as a structure.
fn record(data: &Dataset, xref: &str) -> Structure {
    data.to_structures()
        .into_iter()
        .find(|s| s.xref.as_deref() == Some(xref))
        .unwrap_or_else(|| panic!("no record {xref}"))
}

/// Asserts that strict mode refuses `text` as not conformant.
fn assert_strict_rejects(text: &str) {
    match GedcomBuilder::new().strict(true).build_from_str(text) {
        Err(GedcomError::NonConformant(deviations)) => assert!(!deviations.is_empty()),
        other => panic!("strict mode accepted {text:?}: {other:?}"),
    }
}

/// Asserts that strict mode reads `text` as leniently as the default.
fn assert_strict_accepts(text: &str) {
    let strict = GedcomBuilder::new().strict(true).build_from_str(text);
    match strict {
        Ok(data) => assert_eq!(data, Dataset::parse(text)),
        Err(e) => panic!("strict mode refused {text:?}: {e}"),
    }
}

const MINIMAL_7: &str = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n";

// ----------------------------------------------------------------------------
// Input API
// ----------------------------------------------------------------------------

#[test]
fn every_input_reads_to_the_same_dataset() {
    let text = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX F\n\
                0 @I2@ INDI\n1 NAME Bob /Example/\n1 SEX M\n0 @F1@ FAM\n1 HUSB @I2@\n\
                1 WIFE @I1@\n0 TRLR\n";
    let data = Dataset::parse(text);
    let builder = GedcomBuilder::new();
    assert_eq!(builder.build_from_str(text).unwrap(), data);
    assert_eq!(builder.build_from_bytes(text.as_bytes()).unwrap(), data);
    assert_eq!(builder.build_from_reader(text.as_bytes()).unwrap(), data);
    assert_eq!(Dataset::from_bytes(text.as_bytes()), data);
    assert_eq!(
        stream(text.as_bytes()).to_structures(),
        data.to_structures()
    );

    // A dataset is a value: cloned and read twice, it is equal.
    assert_eq!(data.clone(), data);
    assert_eq!(Dataset::parse(text), data);
}

#[test]
fn simple_fixture_is_read() {
    let text = std::fs::read_to_string("tests/fixtures/simple.ged").unwrap();
    let data = Dataset::parse(text);
    assert_eq!(data.individuals().len(), 3);
    assert_eq!(data.families().len(), 1);
    assert_eq!(data.submitters().len(), 1);

    let header = data.header().unwrap();
    assert_eq!(header.charset.as_ref().unwrap().value, CharacterSet::Ascii);
    assert_eq!(
        header.submitter.map(|s| data.store().xref(s)),
        Some("@SUBMITTER@")
    );
    assert_eq!(data.declared_version(), Some("5.5"));

    assert_eq!(name(&data), "/Father/");
    let address_record = data.submitters().get(0).unwrap().into_owned();
    let address = address_record.address.as_ref().unwrap();
    assert_eq!(
        address.value.to_str(&data),
        "Submitters address\naddress continued here"
    );
    let events_record = data.families().get(0).unwrap().into_owned();
    let events = &events_record.events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Marriage);
    assert_eq!(
        events[0].date.as_ref().unwrap().value.to_str(&data),
        "1 APR 1950"
    );
}

#[test]
fn large_fixture_is_read() {
    let bytes = std::fs::read("tests/fixtures/washington.ged").unwrap();
    let data = Dataset::from_bytes(bytes);
    assert_eq!(data.individuals().len(), 538);
    assert_eq!(data.families().len(), 278);
    assert_eq!(data.declared_version(), Some("5.5.1"));
    assert_eq!(data.version(), GedcomVersion::V5_5_1);
    let header = data.header().unwrap();
    assert_eq!(header.charset.as_ref().unwrap().value, CharacterSet::Utf8);
    let events_record = data.families().get(0).unwrap().into_owned();
    let events = &events_record.events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Marriage);
    assert!(events[0].date.is_some());
}

#[test]
fn every_record_type_is_read_with_its_identifier() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @U1@ SUBM\n1 NAME Example Submitter\n\
         0 @I1@ INDI\n1 NAME Ann /Example/\n0 @F1@ FAM\n0 @R1@ REPO\n1 NAME Sample archive\n\
         0 @S1@ SOUR\n1 TITL Sample register\n0 @M1@ OBJE\n1 FILE photo.jpg\n\
         0 @N1@ NOTE A shared note\n0 TRLR\n",
    );
    let xref = |id| data.store().xref(id);
    assert_eq!(data.submitters().len(), 1);
    assert_eq!(
        xref(data.submitters().get(0).unwrap().into_owned().xref.unwrap()),
        "@U1@"
    );
    assert_eq!(
        xref(
            data.individuals()
                .get(0)
                .unwrap()
                .into_owned()
                .xref
                .unwrap()
        ),
        "@I1@"
    );
    assert_eq!(
        xref(data.families().get(0).unwrap().into_owned().xref.unwrap()),
        "@F1@"
    );
    assert_eq!(
        xref(
            data.repositories()
                .get(0)
                .unwrap()
                .into_owned()
                .xref
                .unwrap()
        ),
        "@R1@"
    );
    assert_eq!(
        xref(data.sources().get(0).unwrap().into_owned().xref.unwrap()),
        "@S1@"
    );
    assert_eq!(
        xref(data.multimedia().get(0).unwrap().into_owned().xref.unwrap()),
        "@M1@"
    );
    assert_eq!(
        xref(data.notes().get(0).unwrap().into_owned().xref.unwrap()),
        "@N1@"
    );
    assert!(data.extra().is_empty());
    // The header and seven records; the trailer is not a record.
    assert_eq!(data.records().count(), 8);
}

#[test]
fn individuals_and_families_are_read() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @PERSON1@ INDI\n1 NAME Ann Example\n1 SEX M\n\
         0 @PERSON2@ INDI\n0 @PERSON3@ INDI\n0 @F1@ FAM\n1 HUSB @PERSON1@\n1 WIFE @PERSON2@\n\
         1 CHIL @PERSON3@\n0 TRLR\n",
    );
    let person = data.find_individual("@PERSON1@").unwrap().into_owned();
    assert_eq!(person.names[0].value.to_str(&data), "Ann Example");
    assert_eq!(person.sex, Some(Sex::Male));

    let family = &data.families().get(0).unwrap().into_owned();
    let xref = |id| data.store().xref(id);
    assert_eq!(xref(family.xref.unwrap()), "@F1@");
    assert_eq!(family.husband_id().map(xref), Some("@PERSON1@"));
    assert_eq!(family.wife_id().map(xref), Some("@PERSON2@"));
    let children: Vec<&str> = family
        .children
        .iter()
        .map(|c| xref(c.individual.unwrap()))
        .collect();
    assert_eq!(children, ["@PERSON3@"]);
}

#[test]
fn file_size_limit_refuses_a_larger_input() {
    let large = "0 HEAD\n".to_string() + &"X".repeat(1000);
    let builder = GedcomBuilder::new().max_file_size(100);
    assert!(matches!(
        builder.build_from_str(large.as_str()),
        Err(GedcomError::FileTooLarge {
            size: 1007,
            max: 100
        })
    ));
    assert!(matches!(
        builder.build_from_bytes(large.as_bytes()),
        Err(GedcomError::FileTooLarge { max: 100, .. })
    ));
    assert!(
        GedcomBuilder::new()
            .max_file_size(2000)
            .build_from_str(large)
            .is_ok()
    );
}

// ----------------------------------------------------------------------------
// Leniency: malformed input is read, strict mode refuses it
// ----------------------------------------------------------------------------

#[test]
fn minimal_file_conforms() {
    assert_strict_accepts(MINIMAL_7);
    let data = Dataset::parse(MINIMAL_7);
    assert!(data.header().is_some());
    assert_eq!(data.record_count(), 1);
    assert_eq!(data.version(), GedcomVersion::V7_0);
}

#[test]
fn last_line_without_terminator_reads_the_same() {
    let without = MINIMAL_7.trim_end();
    // 7.0 requires a terminator after the last line.
    assert_strict_rejects(without);
    assert_eq!(
        Dataset::parse(without).to_structures(),
        Dataset::parse(MINIMAL_7).to_structures()
    );
}

#[test]
fn missing_header_or_trailer_keeps_the_records() {
    for text in [
        "0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR",
        "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/",
        "0 @I1@ INDI\n1 NAME Ann /Example/",
    ] {
        let data = Dataset::parse(text);
        assert_eq!(data.individuals().len(), 1, "{text:?}");
        assert_eq!(name(&data), "Ann /Example/", "{text:?}");
        assert!(data.extra().is_empty(), "{text:?}");
    }
    // A file without a header does not conform.
    assert_strict_rejects("0 @I1@ INDI\n1 NAME Ann /Example/\n0 TRLR\n");
}

#[test]
fn empty_or_blank_input_is_an_empty_dataset() {
    for text in ["", "   \n\n  ", "\n\r\n\t\n"] {
        let data = Dataset::parse(text);
        assert!(data.header().is_none(), "{text:?}");
        assert_eq!(data.record_count(), 0, "{text:?}");
        assert_eq!(read(text.as_bytes()), data, "{text:?}");
    }
}

#[test]
fn unknown_record_is_kept() {
    let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 UNKNOWN_TOP_LEVEL_TAG\n0 TRLR\n";
    let data = Dataset::parse(text);
    assert_eq!(data.extra().len(), 1);
    assert_eq!(
        data.store()
            .tag(data.extra().get(0).unwrap().into_owned().tag),
        "UNKNOWN_TOP_LEVEL_TAG"
    );
    assert!(write(&data).contains("UNKNOWN_TOP_LEVEL_TAG"));
    assert_strict_rejects(text);
}

#[test]
fn unparsable_date_is_kept_as_written() {
    let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 BIRT\n2 DATE not-valid\n0 TRLR\n";
    let data = Dataset::parse(text);
    let birth_record = data.individuals().get(0).unwrap().into_owned();
    let birth = birth_record.birth().unwrap();
    assert_eq!(
        birth.date.as_ref().unwrap().value.to_str(&data),
        "not-valid"
    );
    assert_strict_rejects(text);
}

#[test]
fn reference_to_a_missing_record_is_kept_and_reported() {
    let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @F1@ FAM\n1 HUSB @I999@\n0 TRLR\n";
    let data = Dataset::parse(text);
    let husband_record = data.families().get(0).unwrap().into_owned();
    let husband = husband_record.husband_id().unwrap();
    assert_eq!(data.store().xref(husband), "@I999@");
    let dangling = data.dangling_references();
    assert_eq!(dangling.len(), 1);
    assert_eq!(data.store().xref(dangling[0].pointer), "@I999@");
    assert_strict_rejects(text);

    let valid = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n0 @F1@ FAM\n1 HUSB @I1@\n0 TRLR\n";
    assert!(Dataset::parse(valid).dangling_references().is_empty());
    assert_strict_accepts(valid);
}

#[test]
fn duplicate_identifiers_keep_both_records_and_find_the_first() {
    let text = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
                0 @I1@ INDI\n1 NAME Bob /Example/\n0 TRLR\n";
    let data = Dataset::parse(text);
    assert_eq!(data.individuals().len(), 2);
    let found = data.find_individual("@I1@").unwrap().into_owned();
    assert_eq!(found.names[0].value.to_str(&data), "Ann /Example/");
    assert_strict_rejects(text);
}

#[test]
fn family_without_members_is_read() {
    let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @F1@ FAM\n0 TRLR\n");
    assert_eq!(data.families().len(), 1);
    assert!(
        data.families()
            .get(0)
            .unwrap()
            .into_owned()
            .husband
            .is_none()
    );
    assert!(
        data.families()
            .get(0)
            .unwrap()
            .into_owned()
            .children
            .is_empty()
    );
}

// ----------------------------------------------------------------------------
// Encodings and line terminators, in memory and streaming
// ----------------------------------------------------------------------------

/// A 5.5.1 file with one individual named `name`, `CHAR` set to `char`.
fn file(char: &str, name: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n2 FORM LINEAGE-LINKED\n1 CHAR {char}\n0 @I1@ INDI\n1 NAME "
    )
    .into_bytes();
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(b" /Exemple/\n0 TRLR\n");
    bytes
}

fn with_cr(bytes: Vec<u8>) -> Vec<u8> {
    bytes
        .into_iter()
        .map(|b| if b == b'\n' { b'\r' } else { b })
        .collect()
}

/// Reads `bytes` both ways and checks the name and the reported encoding.
fn check(bytes: &[u8], want_name: &str, want_encoding: GedcomEncoding) {
    let memory = read(bytes);
    assert_eq!(name(&memory), want_name);
    assert_eq!(stream(bytes).to_structures(), memory.to_structures());
    assert_eq!(decode(bytes).encoding, want_encoding);
    assert_eq!(
        DecodeReader::new(bytes).unwrap().encoding(),
        want_encoding,
        "stream encoding"
    );
}

#[test]
fn enc_ansel_combining_is_composed() {
    let bytes = file("ANSEL", b"Andr\xE2e");
    check(&bytes, "André /Exemple/", GedcomEncoding::Ansel);
    check(&with_cr(bytes), "André /Exemple/", GedcomEncoding::Ansel);
}

#[test]
fn enc_ansel_mark_split_by_conc() {
    let bytes = b"0 HEAD\r\n1 CHAR ANSEL\r\n0 @I1@ INDI\r\n1 NOTE Andr\xE2\r\n2 CONC e lives here\r\n0 TRLR\r\n";
    let data = read(bytes);
    assert_eq!(
        note_text(
            &data,
            &data.individuals().get(0).unwrap().into_owned().notes[0]
        ),
        "André lives here"
    );
    assert_eq!(stream(bytes).to_structures(), data.to_structures());
}

#[test]
fn enc_single_byte_code_pages() {
    for (char, name, encoding) in [
        ("ANSI", &b"Andr\xE9"[..], GedcomEncoding::Windows1252),
        ("ASCII", b"Andr\xE9", GedcomEncoding::Windows1252),
        ("ISO-8859-1", b"Andr\xE9", GedcomEncoding::Iso8859_1),
        ("MACINTOSH", b"Andr\x8E", GedcomEncoding::MacRoman),
        ("IBMPC", b"Andr\x82", GedcomEncoding::Cp437),
    ] {
        check(&file(char, name), "André /Exemple/", encoding);
    }
}

#[test]
fn enc_cp1252_euro() {
    check(
        &file("WINDOWS-1252", b"Prix 5\x80 \x91q\x92"),
        "Prix 5€ ‘q’ /Exemple/",
        GedcomEncoding::Windows1252,
    );
    check(
        &file("cp1252", b"Prix 5\x80"),
        "Prix 5€ /Exemple/",
        GedcomEncoding::Windows1252,
    );
}

#[test]
fn enc_undeclared_latin1() {
    let bytes = b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Andr\xE9 /Exemple/\n0 TRLR\n";
    check(bytes, "André /Exemple/", GedcomEncoding::Windows1252);
}

#[test]
fn enc_utf8_mislabelled_unicode_and_ansel() {
    for char in ["UNICODE", "ANSEL"] {
        check(
            &file(char, "André".as_bytes()),
            "André /Exemple/",
            GedcomEncoding::Utf8,
        );
    }
}

#[test]
fn enc_utf8_cr_and_bom_crlf() {
    let cr = with_cr(file("UTF-8", "André".as_bytes()));
    check(&cr, "André /Exemple/", GedcomEncoding::Utf8);
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend_from_slice(
        "0 HEAD\r\n1 GEDC\r\n2 VERS 7.0\r\n0 @I1@ INDI\r\n1 NAME André /Exemple/\r\n0 TRLR\r\n"
            .as_bytes(),
    );
    check(&bom, "André /Exemple/", GedcomEncoding::Utf8);
}

#[test]
fn enc_utf16_with_and_without_bom() {
    let text = String::from_utf8(file("UNICODE", "André".as_bytes())).unwrap();
    for eol in ["\n", "\r"] {
        let text = text.replace('\n', eol);
        let le = encode(&text, GedcomEncoding::Utf16Le).unwrap();
        let be = encode(&text, GedcomEncoding::Utf16Be).unwrap();
        check(&le, "André /Exemple/", GedcomEncoding::Utf16Le);
        check(&be, "André /Exemple/", GedcomEncoding::Utf16Be);
        // Without the byte order mark.
        check(&le[2..], "André /Exemple/", GedcomEncoding::Utf16Le);
        check(&be[2..], "André /Exemple/", GedcomEncoding::Utf16Be);
    }
}

#[test]
fn eol_crlf_lfcr_and_mixed() {
    let crlf =
        b"0 HEAD\r\n1 GEDC\r\n2 VERS 7.0\r\n0 @I1@ INDI\r\n1 NAME Ann /Example/\r\n0 TRLR\r\n";
    assert_strict_accepts(std::str::from_utf8(crlf).unwrap());
    let lfcr = b"0 HEAD\n\r1 GEDC\n\r2 VERS 5.5.1\n\r1 CHAR UTF-8\n\r0 @I1@ INDI\n\r1 NAME Ann /Example/\n\r0 TRLR\n\r";
    let mixed =
        b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 CHAR UTF-8\n0 @I1@ INDI\r1 NAME Ann /Example/\n0 TRLR\n";
    for bytes in [&crlf[..], lfcr, mixed] {
        let data = read(bytes);
        assert_eq!(name(&data), "Ann /Example/");
        assert_eq!(stream(bytes).to_structures(), data.to_structures());
    }
}

#[test]
fn blank_and_whitespace_only_lines_are_skipped() {
    let bytes = b"\n0 HEAD\n1 GEDC\n2 VERS 7.0\n\n0 @I1@ INDI\n  \t\n1 SEX M\n\r\n0 TRLR\n\n";
    let data = read(bytes);
    assert_eq!(
        data.individuals().get(0).unwrap().into_owned().sex,
        Some(Sex::Male)
    );
    assert!(
        data.individuals()
            .get(0)
            .unwrap()
            .into_owned()
            .extra
            .is_empty()
    );
    assert_eq!(stream(bytes).to_structures(), data.to_structures());
}

#[test]
fn empty_cont_with_trailing_delimiter_in_crlf() {
    // `2 CONT ` then CRLF does not swallow the next line.
    let bytes = b"0 HEAD\r\n1 GEDC\r\n2 VERS 5.5.1\r\n0 @I1@ INDI\r\n1 NOTE first\r\n2 CONT \r\n2 CONT third\r\n0 TRLR\r\n";
    let data = read(bytes);
    assert_eq!(
        note_text(
            &data,
            &data.individuals().get(0).unwrap().into_owned().notes[0]
        ),
        "first\n\nthird"
    );
    assert_eq!(stream(bytes).to_structures(), data.to_structures());
}

/// One fictitious body in six encodings and three line terminators reads to
/// the same model, in memory and streaming.
#[test]
fn encoding_and_terminator_matrix() {
    let body = "0 @I1@ INDI\n1 NAME Zoé /Straße/\n1 SEX F\n1 BIRT\n2 DATE 1 JAN 1900\n\
                2 PLAC Ærøskøbing\n1 NOTE Première ligne\n2 CONT seconde ligne\n\
                0 @I2@ INDI\n1 NAME Jürgen /Ødegård/\n0 @F1@ FAM\n1 WIFE @I1@\n1 HUSB @I2@\n0 TRLR\n";
    let mut reference: Option<(Vec<Structure>, Dataset)> = None;
    // UTF-16 output carries its own byte order mark; `bom` adds the UTF-8 one.
    for (label, encoding, bom) in [
        ("UTF-8", GedcomEncoding::Utf8, false),
        ("UTF-8", GedcomEncoding::Utf8, true),
        ("UNICODE", GedcomEncoding::Utf16Le, false),
        ("UNICODE", GedcomEncoding::Utf16Be, false),
        ("ANSEL", GedcomEncoding::Ansel, false),
        ("ISO-8859-1", GedcomEncoding::Iso8859_1, false),
    ] {
        for eol in ["\n", "\r\n", "\r"] {
            let text =
                format!("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 CHAR {label}\n{body}").replace('\n', eol);
            let mut bytes = if bom {
                vec![0xEF, 0xBB, 0xBF]
            } else {
                Vec::new()
            };
            bytes.extend(encode(&text, encoding).unwrap());
            let mut data = read(&bytes);
            assert_eq!(
                stream(&bytes).to_structures(),
                data.to_structures(),
                "{label} {eol:?}"
            );
            // Only the declared character set differs.
            data.header_mut().unwrap().charset = None;
            let structures = data.to_structures();
            match &reference {
                None => reference = Some((structures, data)),
                Some((r, _)) => assert_eq!(&structures, r, "{label} {eol:?}"),
            }
        }
    }
    let (_, r) = reference.unwrap();
    assert_eq!(name(&r), "Zoé /Straße/");
    assert_eq!(
        note_text(&r, &r.individuals().get(0).unwrap().into_owned().notes[0]),
        "Première ligne\nseconde ligne"
    );
}

// ----------------------------------------------------------------------------
// Continuation lines
// ----------------------------------------------------------------------------

#[test]
fn spaces_after_the_conc_and_cont_delimiter_are_kept() {
    // One space separates the tag from its value; any further space is
    // part of the value.
    let original = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @F1@ FAM\n1 NOTE first\n2 CONC  second\n\
                    2 CONT    indented line\n0 TRLR";
    for data in [Dataset::parse(original), stream(original.as_bytes())] {
        assert_eq!(
            note_text(
                &data,
                &data.families().get(0).unwrap().into_owned().notes[0]
            ),
            "first second\n   indented line"
        );
    }

    let mut parser = GedcomStreamParser::new(BufReader::new(original.as_bytes())).unwrap();
    let family = parser
        .find_map(|r| {
            let r = r.unwrap();
            match r.record() {
                RecordRef::Family(f) => {
                    Some(note_text_in(&r, &f.notes().first().unwrap().into_owned()))
                }
                _ => None,
            }
        })
        .unwrap();
    assert_eq!(family, "first second\n   indented line");
}

/// The text of an inline note, from any store.
fn note_text_in(store: &impl AsRef<oxidged::model::Store>, note: &Note) -> String {
    match &note.content {
        NoteContent::Text(text) => text.to_str(store).into_owned(),
        NoteContent::Shared(_) => panic!("shared note"),
    }
}

#[test]
fn long_text_round_trip_keeps_spaces_at_conc_splits() {
    // Words of every length around the 255-byte limit, so that the naive
    // split point falls next to a space in some of them.
    for word_len in 1..12 {
        let word = "x".repeat(word_len);
        let text = vec![word.as_str(); 120].join(" ");
        let original = format!("0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @F1@ FAM\n1 NOTE {text}\n0 TRLR");
        let data1 = Dataset::parse(original);
        let written = write(&data1);

        for line in written.lines().filter(|l| l.contains(" CONC ")) {
            let value = line.split_once(" CONC ").unwrap().1;
            assert!(
                !value.starts_with(' '),
                "CONC value starts with a space: {line:?}"
            );
        }
        for line in written.lines() {
            assert!(!line.ends_with(' '), "line ends with a space: {line:?}");
        }

        let data2 = Dataset::parse(written);
        assert_eq!(
            note_text(
                &data2,
                &data2.families().get(0).unwrap().into_owned().notes[0]
            ),
            text,
            "word length {word_len}"
        );
    }
}

#[test]
fn event_and_attribute_values_are_continued() {
    // A value continued before the structure's substructures.
    let original = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 TITL Duke\n2 CONC  of Somewhere\n\
                    1 OCCU Long\n2 CONT second line\n2 DATE 1900\n1 BIRT Born at home\n\
                    2 CONC  during a storm\n2 DATE 1 JAN 1900\n0 TRLR";
    let data = Dataset::parse(original);
    let events_record = data.individuals().get(0).unwrap().into_owned();
    let events = &events_record.events;
    assert_eq!(events[0].kind, EventKind::Title);
    assert_eq!(events[0].value.to_str(&data), "Duke of Somewhere");
    assert_eq!(events[1].kind, EventKind::Occupation);
    assert_eq!(events[1].value.to_str(&data), "Long\nsecond line");
    assert!(events[1].date.is_some());
    assert_eq!(events[2].kind, EventKind::Birth);
    assert_eq!(events[2].value.to_str(&data), "Born at home during a storm");
    assert!(events.iter().all(|e| e.extra.is_empty()));
}

#[test]
fn extension_tags_after_continued_text_are_substructures() {
    let original = "0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @N1@ SNOTE First\n1 CONT second\n1 _X kept\n\
                    0 @I1@ INDI\n1 NOTE a\n2 CONT b\n2 _Y kept\n0 TRLR";
    let data = Dataset::parse(original);
    let shared = &data.notes().get(0).unwrap().into_owned();
    assert_eq!(shared.text.to_str(&data), "First\nsecond");
    assert_eq!(data.store().tag(shared.extra[0].tag), "_X");
    let note_record = data.individuals().get(0).unwrap().into_owned();
    let note = &note_record.notes[0];
    assert_eq!(note_text(&data, note), "a\nb");
    assert_eq!(data.store().tag(note.extra[0].tag), "_Y");
}

#[test]
fn long_values_of_any_tag_round_trip() {
    // The writer splits any value over the line length with CONC, and any
    // value with a newline with CONT; reading puts each one back whole, not
    // only the values of the tags that usually carry long text. No space
    // near the split points: keeping spaces at a CONC split is a separate
    // matter.
    let long = |label: &str| format!("{label}:{}", "abcdefghij".repeat(30));
    let event_type = long("Event type");
    let agency = long("Agency");
    let relation = long("Relation");
    let title = long("Title");
    let repo_name = long("Archive");
    let multi_line_type = "First line\nSecond line";

    let mut data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 EVEN\n2 TYPE x\n2 AGNC x\n1 ASSO @I2@\n\
         2 RELA x\n1 FACT x\n2 TYPE x\n0 @I2@ INDI\n0 @M1@ OBJE\n1 FILE a.jpg\n2 FORM jpg\n\
         2 TITL x\n0 @R1@ REPO\n1 NAME x\n0 TRLR",
    );
    {
        let mut person = data.get_mut_at::<Individual>(0).unwrap();
        person.events[0].detail_mut().classification = Some(Text::new(event_type.as_str()));
        person.events[0].detail_mut().agency = Some(Text::new(agency.as_str()));
        person.detail_mut().associations[0].relation = Some(Text::new(relation.as_str()));
        person.events[1].detail_mut().classification = Some(Text::new(multi_line_type));
    }
    data.get_mut_at::<Multimedia>(0).unwrap().files[0].title = Some(Text::new(title.as_str()));
    data.get_mut_at::<Repository>(0).unwrap().name = Some(Text::new(repo_name.as_str()));

    let written = write(&data);
    assert!(written.contains(" CONC "), "{written}");
    assert!(written.contains("3 CONT Second line"), "{written}");

    let reread = Dataset::parse(written);
    let person = &reread.individuals().get(0).unwrap().into_owned();
    let t = |x: Option<&Text>| text(&reread, x);
    assert_eq!(
        t(person.events[0].detail().classification.as_ref()),
        Some(event_type)
    );
    assert_eq!(t(person.events[0].detail().agency.as_ref()), Some(agency));
    assert_eq!(
        t(person.detail().associations[0].relation.as_ref()),
        Some(relation)
    );
    assert_eq!(
        t(person.events[1].detail().classification.as_ref()).as_deref(),
        Some(multi_line_type)
    );
    assert_eq!(
        t(reread.multimedia().get(0).unwrap().into_owned().files[0]
            .title
            .as_ref()),
        Some(title)
    );
    assert_eq!(
        t(reread
            .repositories()
            .get(0)
            .unwrap()
            .into_owned()
            .name
            .as_ref()),
        Some(repo_name)
    );
}

#[test]
fn note_record_continuations_round_trip() {
    let sample = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @N1@ NOTE\n\
                  1 CONC Ann Example was born Ann Sample. Her last name wa\n\
                  1 CONC s legally\n\
                  1 CONT changed to Example in 1950 in Sampleton.\n0 TRLR";
    let expected = "Ann Example was born Ann Sample. Her last name was legally\n\
                    changed to Example in 1950 in Sampleton.";
    let data = Dataset::parse(sample);
    assert_eq!(data.notes().len(), 1);
    assert_eq!(
        data.notes().get(0).unwrap().into_owned().text.to_str(&data),
        expected
    );

    let written = GedcomWriter::new()
        .gedcom_version(GedcomVersion::V5_5_1)
        .write_to_string(&data)
        .unwrap();
    assert!(
        written.contains("0 @N1@ NOTE Ann Example was born"),
        "{written}"
    );
    let reread = Dataset::parse(written);
    assert_eq!(
        reread
            .notes()
            .get(0)
            .unwrap()
            .into_owned()
            .text
            .to_str(&reread),
        expected
    );
}

#[test]
fn carriage_returns_in_values_are_written_as_line_breaks() {
    // A carriage return inside a value, alone or before a line feed, is a
    // line break: the writer continues the value on a `CONT` line.
    let mut data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
         1 NOTE placeholder\n0 @S1@ SOUR\n1 TITL Parish register\n\
         0 @R1@ REPO\n1 NAME Example archive\n0 TRLR",
    );
    data.get_mut_at::<Individual>(0).unwrap().notes[0].content =
        NoteContent::Text(Text::new("First line\rSecond line\r\nThird line"));
    data.get_mut_at::<Source>(0).unwrap().publication =
        Some(Text::new("Example Press\r\nSample Town"));
    data.get_mut_at::<Repository>(0).unwrap().address = Some(Address {
        value: Text::new("1 Example Street\rSample Town"),
        ..Address::default()
    });

    for (line_ending, eol) in [
        (oxidged::LineEnding::Lf, "\n"),
        (oxidged::LineEnding::CrLf, "\r\n"),
    ] {
        let written = GedcomWriter::new()
            .line_ending(line_ending)
            .write_to_string(&data)
            .unwrap();

        // Every carriage return written is the one of a line ending.
        assert_eq!(
            written.matches('\r').count(),
            written.matches(eol).count() * (eol.len() - 1),
            "{written:?}"
        );
        for expected in [
            format!("1 NOTE First line{eol}2 CONT Second line{eol}2 CONT Third line{eol}"),
            format!("1 PUBL Example Press{eol}2 CONT Sample Town{eol}"),
            format!("1 ADDR 1 Example Street{eol}2 CONT Sample Town{eol}"),
        ] {
            assert!(written.contains(&expected), "{expected:?} in {written:?}");
        }

        let reread = Dataset::parse(written);
        assert_eq!(
            note_text(
                &reread,
                &reread.individuals().get(0).unwrap().into_owned().notes[0]
            ),
            "First line\nSecond line\nThird line"
        );
        assert_eq!(
            text(
                &reread,
                reread
                    .sources()
                    .get(0)
                    .unwrap()
                    .into_owned()
                    .publication
                    .as_ref()
            )
            .as_deref(),
            Some("Example Press\nSample Town")
        );
        let address_record = reread.repositories().get(0).unwrap().into_owned();
        let address = address_record.address.as_ref().unwrap();
        assert_eq!(
            address.value.to_str(&reread),
            "1 Example Street\nSample Town"
        );
    }
}

#[test]
fn carriage_returns_in_a_shared_note_record_are_written_as_line_breaks() {
    let mut data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @N1@ SNOTE placeholder\n0 TRLR");
    data.get_mut_at::<SharedNote>(0).unwrap().text = Text::new("First line\r\n\r\nThird line\r");

    let written = write(&data);
    assert!(!written.contains('\r'), "{written:?}");
    assert!(
        written.contains("0 @N1@ SNOTE First line\n1 CONT\n1 CONT Third line\n1 CONT\n"),
        "{written:?}"
    );
    let reread = Dataset::parse(written);
    assert_eq!(
        reread
            .notes()
            .get(0)
            .unwrap()
            .into_owned()
            .text
            .to_str(&reread),
        "First line\n\nThird line\n"
    );
}

// ----------------------------------------------------------------------------
// The `@@` escape
// ----------------------------------------------------------------------------

#[test]
fn leading_at_sign_round_trip() {
    let original = "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @I1@ INDI\n1 NAME Ann /Example/\n1 BIRT\n\
                    2 DATE @#DJULIAN@ 1700\n2 SOUR @S1@\n0 @F1@ FAM\n1 WIFE @I1@\n\
                    1 NOTE @@home in the village\n2 CONT @@noon every day\n0 @S1@ SOUR\n\
                    1 TITL Register\n0 TRLR";

    let data1 = Dataset::parse(original);
    assert_eq!(
        note_text(
            &data1,
            &data1.families().get(0).unwrap().into_owned().notes[0]
        ),
        "@home in the village\n@noon every day"
    );

    let written = write(&data1);
    for expected in [
        "1 NOTE @@home in the village\n2 CONT @@noon every day\n",
        // Pointers and calendar escapes are not text.
        "2 DATE @#DJULIAN@ 1700\n",
        "2 SOUR @S1@\n",
    ] {
        assert!(
            written.contains(expected),
            "missing {expected:?} in written output:\n{written}"
        );
    }

    let data2 = Dataset::parse(written);
    for xref in ["@I1@", "@F1@", "@S1@"] {
        assert_eq!(record(&data1, xref), record(&data2, xref), "{xref}");
    }
}

#[test]
fn interior_at_signs_are_kept_as_read() {
    let data = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n0 @F1@ FAM\n1 NOTE mail sample@@example.org\n0 TRLR",
    );
    let written = write(&data);
    assert!(
        written.contains("1 NOTE mail sample@@example.org\n"),
        "{written}"
    );
}
