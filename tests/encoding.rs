//! Reading GEDCOM bytes of every encoding: UTF-8 (with and without a byte
//! order mark), UTF-16 LE and BE, ISO-8859-1, ISO-8859-15, Windows-1252
//! (`CHAR ANSI`) and ANSEL; the detection of the encoding from the byte
//! order mark, the bytes and `HEAD.CHAR`; and the encoder. Every name and
//! place is fictitious.

use oxidged::encoding::{GedcomEncoding, decode, detect_encoding, encode};
use oxidged::model::NoteContent;
use oxidged::{Dataset, GedcomBuilder, GedcomError};

/// A minimal 5.5.1 file declaring `char_tag` whose individual has `name`.
fn file_with_name(name: &str, char_tag: &str) -> String {
    format!(
        "0 HEAD\n\
         1 GEDC\n\
         2 VERS 5.5.1\n\
         1 CHAR {char_tag}\n\
         0 @I1@ INDI\n\
         1 NAME {name}\n\
         0 TRLR\n"
    )
}

fn read(bytes: impl Into<Vec<u8>>) -> Dataset {
    GedcomBuilder::new().build_from_bytes(bytes).unwrap()
}

/// The first name of the `index`th individual, as written.
fn name(data: &Dataset, index: usize) -> String {
    data.individuals().get(index).unwrap().into_owned().names[0]
        .value
        .to_str(data)
        .into_owned()
}

/// The text of the first note of the first individual.
fn note(data: &Dataset) -> String {
    match &data.individuals().get(0).unwrap().into_owned().notes[0].content {
        NoteContent::Text(text) => text.to_str(data).into_owned(),
        NoteContent::Shared(_) => panic!("a shared note"),
    }
}

// UTF-8

#[test]
fn utf8_without_bom() {
    let data = read(file_with_name("Zoé /Exámple/", "UTF-8"));
    assert_eq!(data.individuals().len(), 1);
    assert_eq!(name(&data, 0), "Zoé /Exámple/");
}

#[test]
fn utf8_with_bom() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(file_with_name("Jörg /Müstermann/", "UTF-8").as_bytes());
    let data = read(bytes);
    assert_eq!(data.individuals().len(), 1);
    assert_eq!(name(&data, 0), "Jörg /Müstermann/");
}

#[test]
fn utf8_chinese_and_cyrillic_characters() {
    for sample in ["示例 /测试/", "Иван /Примеров/"] {
        let data = read(file_with_name(sample, "UTF-8"));
        assert_eq!(name(&data, 0), sample);
    }
}

#[test]
fn utf8_emoji_in_gedcom_7() {
    // GEDCOM 7.0 allows any Unicode character.
    let content = "0 HEAD\n\
                   1 GEDC\n\
                   2 VERS 7.0\n\
                   0 @I1@ INDI\n\
                   1 NAME Ann /Example/\n\
                   1 NOTE Family reunion 🎉👨‍👩‍👧‍👦\n\
                   0 TRLR\n";
    let data = read(content);
    assert_eq!(data.individuals().len(), 1);
    assert_eq!(note(&data), "Family reunion 🎉👨‍👩‍👧‍👦");
}

// ISO-8859-1 (Latin-1)

#[test]
fn iso8859_1_accented_letters() {
    // é (0xE9), á (0xE1), ü (0xFC), ö (0xF6), ç (0xE7), ø (0xF8), Å (0xC5).
    for (bytes, expected) in [
        (&b"Zo\xE9 /Ex\xE1mple/"[..], "Zoé /Exámple/"),
        (b"J\xF6rg /M\xFCstermann/", "Jörg /Müstermann/"),
        (b"Ren\xE9e /Fran\xE7aise/", "Renée /Française/"),
        (b"S\xF8ren /\xC5mple/", "Søren /Åmple/"),
    ] {
        let mut file =
            b"0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 CHAR ISO-8859-1\n0 @I1@ INDI\n1 NAME ".to_vec();
        file.extend_from_slice(bytes);
        file.extend_from_slice(b"\n0 TRLR\n");
        let data = read(file);
        assert_eq!(data.individuals().len(), 1);
        assert_eq!(name(&data, 0), expected);
    }
}

#[test]
fn iso8859_1_declared_as_latin1() {
    let bytes: &[u8] = b"0 HEAD\n\
                         1 GEDC\n\
                         2 VERS 5.5.1\n\
                         1 CHAR LATIN1\n\
                         0 @I1@ INDI\n\
                         1 NAME Zo\xE9 /Ex\xE1mple/\n\
                         0 TRLR\n";
    assert_eq!(name(&read(bytes), 0), "Zoé /Exámple/");
}

// ISO-8859-15 (Latin-9)

#[test]
fn iso8859_15_euro_sign() {
    // The euro sign is 0xA4 in ISO-8859-15.
    for label in ["ISO-8859-15", "LATIN9"] {
        let mut bytes = format!(
            "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 CHAR {label}\n\
             0 @I1@ INDI\n1 NAME Ann /Example/\n1 NOTE Cost: 100"
        )
        .into_bytes();
        bytes.extend_from_slice(b"\xA4\n0 TRLR\n");
        let data = read(bytes);
        assert_eq!(note(&data), "Cost: 100€", "{label}");
    }
}

#[test]
fn iso8859_15_oe_ligature() {
    // œ (0xBD) is in ISO-8859-15, not in ISO-8859-1.
    let bytes: &[u8] = b"0 HEAD\n\
                         1 GEDC\n\
                         2 VERS 5.5.1\n\
                         1 CHAR ISO-8859-15\n\
                         0 @I1@ INDI\n\
                         1 NAME Ann /Exampl\xBDuf/\n\
                         0 TRLR\n";
    assert_eq!(name(&read(bytes), 0), "Ann /Examplœuf/");
}

// Windows-1252 (`CHAR ANSI`), carried over from upstream PR #105

/// A 5.5.1 file declaring `CHAR ANSI` whose names hold Windows-1252 bytes:
/// `é` (0xE9, shared with Latin-1), `œ` (0x9C) and `€` (0x80), the latter two
/// only in Windows-1252.
const ANSI_FILE: &[u8] = b"0 HEAD\n\
                           1 GEDC\n\
                           2 VERS 5.5.1\n\
                           1 CHAR ANSI\n\
                           0 @I1@ INDI\n\
                           1 NAME Ren\xE9e /Exampl\x9Cuf/\n\
                           1 NOTE Paid 5 \x80\n\
                           0 TRLR\n";

#[test]
fn ansi_file_is_read_as_windows_1252() {
    let data = read(ANSI_FILE);
    assert_eq!(name(&data, 0), "Renée /Examplœuf/");
    assert_eq!(note(&data), "Paid 5 €");
}

#[test]
fn ansi_bytes_are_decoded_as_windows_1252() {
    assert_eq!(detect_encoding(ANSI_FILE), GedcomEncoding::Windows1252);
    let decoded = decode(ANSI_FILE);
    assert_eq!(decoded.encoding, GedcomEncoding::Windows1252);
    assert_eq!(decoded.declared.as_deref(), Some("ANSI"));
    assert!(decoded.text.contains("1 NAME Renée /Examplœuf/\n"));
    assert!(decoded.text.contains("1 NOTE Paid 5 €\n"));
}

#[test]
fn ansi_file_holding_utf8_is_read_as_utf8() {
    // Some producers declare `ANSI` and write UTF-8: the bytes win.
    let bytes = file_with_name("Renée /Example/", "ANSI").into_bytes();
    assert_eq!(detect_encoding(&bytes), GedcomEncoding::Utf8);
    assert_eq!(name(&read(bytes), 0), "Renée /Example/");
}

#[test]
fn ansi_file_holding_ascii_only() {
    let data = read(file_with_name("Ann /Example/", "ANSI"));
    assert_eq!(name(&data, 0), "Ann /Example/");
}

// UTF-16

#[test]
fn utf16_with_bom() {
    for (sample, encoding) in [
        ("Zoé /Exámple/", GedcomEncoding::Utf16Le),
        ("Jörg /Müstermann/", GedcomEncoding::Utf16Be),
        ("示例 /测试/", GedcomEncoding::Utf16Le),
        ("Иван /Примеров/", GedcomEncoding::Utf16Be),
    ] {
        let bytes = encode(&file_with_name(sample, "UNICODE"), encoding).unwrap();
        let data = read(bytes);
        assert_eq!(data.individuals().len(), 1);
        assert_eq!(name(&data, 0), sample, "{encoding}");
    }
}

#[test]
fn utf16_round_trips_through_the_encoder() {
    for (sample, encoding) in [
        ("日本語 /テスト/", GedcomEncoding::Utf16Le),
        ("Ελληνικά /Κείμενο/", GedcomEncoding::Utf16Be),
    ] {
        let original = file_with_name(sample, "UTF-16");
        let bytes = encode(&original, encoding).unwrap();
        let decoded = decode(&bytes);
        assert_eq!(decoded.encoding, encoding);
        assert_eq!(decoded.text, original);
        assert_eq!(decoded.declared.as_deref(), Some("UTF-16"));
        assert_eq!(name(&read(bytes), 0), sample);
    }
}

// Detection

#[test]
fn detection_by_byte_order_mark() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"0 HEAD\n0 TRLR\n");
    assert_eq!(decode(&bytes).encoding, GedcomEncoding::Utf8);
    for encoding in [GedcomEncoding::Utf16Le, GedcomEncoding::Utf16Be] {
        let bytes = encode("0 HEAD\n0 TRLR\n", encoding).unwrap();
        assert_eq!(decode(&bytes).encoding, encoding);
        assert_eq!(detect_encoding(&bytes), encoding);
    }
}

#[test]
fn detection_by_char_tag() {
    for (label, encoding) in [
        ("UTF-8", GedcomEncoding::Utf8),
        ("ISO-8859-1", GedcomEncoding::Iso8859_1),
        ("ISO-8859-15", GedcomEncoding::Iso8859_15),
        ("ANSEL", GedcomEncoding::Ansel),
    ] {
        let bytes = format!("0 HEAD\n1 CHAR {label}\n0 TRLR\n");
        let decoded = decode(bytes.as_bytes());
        assert_eq!(decoded.encoding, encoding, "{label}");
        assert_eq!(decoded.declared.as_deref(), Some(label));
    }
}

#[test]
fn detection_without_char_is_ascii() {
    let decoded = decode(b"0 HEAD\n1 GEDC\n2 VERS 5.5\n0 TRLR\n");
    assert_eq!(decoded.encoding, GedcomEncoding::Ascii);
    assert_eq!(decoded.declared, None);
}

#[test]
fn char_tag_after_other_header_lines() {
    let bytes: &[u8] = b"0 HEAD\n\
                         1 SOUR EXAMPLE_APP\n\
                         1 GEDC\n\
                         2 VERS 5.5.1\n\
                         1 CHAR ISO-8859-1\n\
                         0 @I1@ INDI\n\
                         1 NAME Zo\xE9 /Ex\xE1mple/\n\
                         0 TRLR\n";
    assert_eq!(name(&read(bytes), 0), "Zoé /Exámple/");
}

#[test]
fn unicode_char_is_utf16() {
    let bytes = encode(
        &file_with_name("Ann /Example/", "UNICODE"),
        GedcomEncoding::Utf16Le,
    )
    .unwrap();
    let data = read(bytes);
    assert_eq!(data.individuals().len(), 1);
    assert_eq!(name(&data, 0), "Ann /Example/");
}

// An explicit encoding

#[test]
fn explicit_encoding() {
    let utf8 = file_with_name("Zoé /Exámple/", "UTF-8");
    let data = GedcomBuilder::new()
        .build_from_bytes_with_encoding(utf8.as_bytes(), GedcomEncoding::Utf8)
        .unwrap();
    assert_eq!(name(&data, 0), "Zoé /Exámple/");

    // No CHAR: the bytes are read in the encoding given.
    let latin1: &[u8] = b"0 HEAD\n\
                          1 GEDC\n\
                          2 VERS 5.5.1\n\
                          0 @I1@ INDI\n\
                          1 NAME Zo\xE9 /Ex\xE1mple/\n\
                          0 TRLR\n";
    let data = GedcomBuilder::new()
        .build_from_bytes_with_encoding(latin1, GedcomEncoding::Iso8859_1)
        .unwrap();
    assert_eq!(name(&data, 0), "Zoé /Exámple/");

    let utf16 = encode(
        &file_with_name("Zoé /Exámple/", "UTF-16"),
        GedcomEncoding::Utf16Le,
    )
    .unwrap();
    let data = GedcomBuilder::new()
        .build_from_bytes_with_encoding(&utf16, GedcomEncoding::Utf16Le)
        .unwrap();
    assert_eq!(name(&data, 0), "Zoé /Exámple/");
}

// ANSEL

#[test]
fn ansel_header_alone() {
    let data = GedcomBuilder::new()
        .build_from_bytes_with_encoding(b"0 HEAD\n1 CHAR ANSEL\n0 TRLR\n", GedcomEncoding::Ansel)
        .unwrap();
    assert!(data.header().is_some());
}

#[test]
fn ansel_combining_marks_are_composed() {
    // ANSEL writes a mark before its letter: acute (0xE2) + e is é.
    let mut bytes = b"0 HEAD\n1 CHAR ANSEL\n0 @I1@ INDI\n1 NAME Zo".to_vec();
    bytes.extend_from_slice(&[0xE2, b'e']);
    bytes.extend_from_slice(b" /Ex");
    bytes.extend_from_slice(&[0xE2, b'a']);
    bytes.extend_from_slice(b"mple/\n0 TRLR\n");
    let data = read(bytes);
    assert_eq!(data.individuals().len(), 1);
    assert_eq!(name(&data, 0), "Zoé /Exámple/");
    assert_eq!(
        data.individuals()
            .get(0)
            .unwrap()
            .into_owned()
            .full_name(&data)
            .as_deref(),
        Some("Zoé Exámple")
    );
}

#[test]
fn ansel_special_letters() {
    // Ł (0xA1), ł (0xB1), Ø (0xA2), ø (0xB2).
    let mut bytes = b"0 HEAD\n1 CHAR ANSEL\n0 @I1@ INDI\n1 NAME ".to_vec();
    bytes.extend_from_slice(&[0xA1, 0xB1, 0xA2, 0xB2]);
    bytes.extend_from_slice(b" /Example/\n0 TRLR\n");
    let data = read(bytes);
    assert_eq!(name(&data, 0), "ŁłØø /Example/");
}

// Limits and larger files

#[test]
fn file_size_limit_applies_to_bytes() {
    let content = file_with_name("Ann /Example/", "UTF-8");
    let error = GedcomBuilder::new()
        .max_file_size(10)
        .build_from_bytes(content.as_bytes())
        .unwrap_err();
    assert!(
        matches!(error, GedcomError::FileTooLarge { size, max: 10 } if size == content.len()),
        "{error}"
    );
}

#[test]
fn complete_iso8859_1_file() {
    let bytes: &[u8] = b"0 HEAD\n\
                         1 SOUR EXAMPLE_APP\n\
                         2 NAME Example Application\n\
                         1 GEDC\n\
                         2 VERS 5.5.1\n\
                         2 FORM LINEAGE-LINKED\n\
                         1 CHAR ISO-8859-1\n\
                         0 @I1@ INDI\n\
                         1 NAME Zo\xE9 /Ex\xE1mple/\n\
                         1 SEX F\n\
                         1 BIRT\n\
                         2 DATE 1 JAN 1950\n\
                         2 PLAC Sampl\xE9ton, Ex\xE1mplia\n\
                         1 FAMS @F1@\n\
                         0 @I2@ INDI\n\
                         1 NAME J\xF6rg /M\xFCstermann/\n\
                         1 SEX M\n\
                         1 FAMS @F1@\n\
                         0 @F1@ FAM\n\
                         1 HUSB @I2@\n\
                         1 WIFE @I1@\n\
                         1 MARR\n\
                         2 DATE 15 JUN 1975\n\
                         2 PLAC Ex\xE1mpleville, Ex\xE1mplia\n\
                         0 TRLR\n";
    let data = read(bytes);
    assert_eq!(data.individuals().len(), 2);
    assert_eq!(data.families().len(), 1);
    assert!(data.dangling_references().is_empty());

    assert_eq!(name(&data, 0), "Zoé /Exámple/");
    let birth_record = data.individuals().get(0).unwrap().into_owned();
    let birth = birth_record.birth().unwrap();
    assert_eq!(
        birth.place.as_ref().unwrap().name.to_str(&data),
        "Sampléton, Exámplia"
    );
    assert_eq!(name(&data, 1), "Jörg /Müstermann/");
    let marriage_record = data.families().get(0).unwrap().into_owned();
    let marriage = &marriage_record.events[0];
    assert_eq!(
        marriage.place.as_ref().unwrap().name.to_str(&data),
        "Exámpleville, Exámplia"
    );

    // ISO-8859-1 is not a character set of the 5.5.1 specification: the
    // strict mode refuses it.
    let Err(GedcomError::NonConformant(deviations)) =
        GedcomBuilder::new().strict(true).build_from_bytes(bytes)
    else {
        panic!("accepted in strict mode");
    };
    assert!(
        deviations
            .iter()
            .any(|d| d.line == 7 && d.detail.contains("ISO-8859-1")),
        "{deviations:?}"
    );
}

#[test]
fn fixtures_read_from_bytes() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let bytes = std::fs::read(dir.join("simple.ged")).unwrap();
    let encoding = decode(&bytes).encoding;
    assert!(
        matches!(encoding, GedcomEncoding::Ascii | GedcomEncoding::Utf8),
        "{encoding:?}"
    );
    let data = read(bytes);
    assert_eq!(data.individuals().len(), 3);
    assert_eq!(data.families().len(), 1);

    let data = read(std::fs::read(dir.join("washington.ged")).unwrap());
    assert_eq!(data.individuals().len(), 538);
    assert_eq!(data.families().len(), 278);
}

// The encoder

#[test]
fn encoder_refuses_what_an_encoding_cannot_hold() {
    let error = encode("Zoë", GedcomEncoding::Ascii).unwrap_err();
    assert_eq!(
        (error.encoding, error.character, error.offset),
        (GedcomEncoding::Ascii, 'ë', 2)
    );
    let error = encode("Ann €", GedcomEncoding::Iso8859_15).map(|b| b.len());
    assert_eq!(error, Ok(5));
    assert!(encode("示例", GedcomEncoding::Windows1252).is_err());
    for encoding in [
        GedcomEncoding::Ansel,
        GedcomEncoding::Windows1252,
        GedcomEncoding::Iso8859_15,
        GedcomEncoding::Utf16Le,
        GedcomEncoding::Utf16Be,
        GedcomEncoding::Utf8,
    ] {
        let bytes = encode("Renée /Exemple/", encoding).unwrap();
        assert_eq!(
            oxidged::encoding::decode_as(&bytes, encoding),
            "Renée /Exemple/",
            "{encoding}"
        );
    }
}
