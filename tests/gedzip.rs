//! GEDZIP archives: reading and writing a dataset and its media through
//! the builder, the reader and writer types and the convenience functions;
//! entry limits, `FILE` reference matching, compression and errors. Every
//! name and value is fictitious.
#![cfg(feature = "gedzip")]

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use oxidged::gedzip::{
    GEDCOM_FILENAME, GedzipError, GedzipReader, GedzipWriter, read_gedzip, write_gedzip,
    write_gedzip_with_media,
};
use oxidged::model::Dataset;
use oxidged::{GedcomBuilder, GedcomError, GedcomVersion, GedcomWriter};

const FAMILY: &str = "0 HEAD\n1 GEDC\n2 VERS 7.0\n\
    0 @I1@ INDI\n1 NAME Ann /Example/\n1 SEX F\n1 OBJE @M1@\n\
    0 @I2@ INDI\n1 NAME Bob /Example/\n1 SEX M\n\
    0 @F1@ FAM\n1 HUSB @I2@\n1 WIFE @I1@\n\
    0 @M1@ OBJE\n1 FILE media/portrait.jpg\n2 FORM image/jpeg\n0 TRLR\n";

/// An archive of a minimal file and these media.
fn archive(media: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    writer
        .write_gedcom_bytes(b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n")
        .unwrap();
    for (name, bytes) in media {
        writer.add_media_file(name, bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn a_dataset_round_trips_through_an_archive() {
    let data = Dataset::parse(FAMILY);
    let bytes = write_gedzip(&data).unwrap();
    let read = read_gedzip(&bytes).unwrap();
    assert_eq!(read.version(), GedcomVersion::V7_0);
    assert_eq!(read.individuals().len(), 2);
    assert_eq!(read.families().len(), 1);
    assert_eq!(read.to_structures(), Dataset::parse(FAMILY).to_structures());
    let ann = read.find_individual("@I1@").unwrap().into_owned();
    assert_eq!(ann.full_name(&read).as_deref(), Some("Ann Example"));

    // The builder reads the same archive.
    let built = GedcomBuilder::new()
        .build_from_gedzip(Cursor::new(&bytes))
        .unwrap();
    assert_eq!(built, read);
}

#[test]
fn the_reader_and_writer_types() {
    let data = Dataset::parse(FAMILY);
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    assert!(!writer.has_gedcom());
    writer.write_dataset(&data).unwrap();
    assert!(writer.has_gedcom());
    writer
        .add_media_file("media/portrait.jpg", b"JPEG BYTES")
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    let mut reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.len(), 2);
    assert!(!reader.is_empty());
    assert_eq!(reader.file_names(), [GEDCOM_FILENAME, "media/portrait.jpg"]);
    assert_eq!(reader.media_files(), ["media/portrait.jpg"]);
    assert!(reader.contains_file(GEDCOM_FILENAME));

    let read = reader.read_dataset(&GedcomBuilder::new()).unwrap();
    // The FILE reference of the dataset finds its entry.
    let path = read.multimedia().get(0).unwrap().into_owned().files[0]
        .path
        .to_str(&read)
        .into_owned();
    assert_eq!(reader.read_media_file(&path).unwrap(), b"JPEG BYTES");

    // gedcom.ged holds what the default writer writes, its byte order mark
    // included.
    let mut written = Vec::new();
    GedcomWriter::new().write(&mut written, &data).unwrap();
    assert_eq!(reader.read_gedcom_bytes().unwrap(), written);
    assert!(written.starts_with("\u{feff}0 HEAD\n".as_bytes()));
}

#[test]
fn an_archive_is_written_in_any_version() {
    let data = Dataset::parse(FAMILY);
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    writer
        .write_dataset_with(
            &data,
            &GedcomWriter::new().gedcom_version(GedcomVersion::V7_1),
        )
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let read = read_gedzip(&bytes).unwrap();
    assert_eq!(read.version(), GedcomVersion::V7_1);
    assert_eq!(read.individuals().len(), 2);
}

#[test]
fn media_are_written_with_the_dataset() {
    let data = Dataset::parse(FAMILY);
    let mut media = HashMap::new();
    media.insert("notes.txt".to_string(), b"Hello, Sampleton!".to_vec());
    media.insert(
        "media/portrait.jpg".to_string(),
        vec![0xFF, 0xD8, 0xFF, 0xE0],
    );
    let bytes = write_gedzip_with_media(&data, &media).unwrap();

    let mut reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.len(), 3);
    let mut names = reader.media_files();
    names.sort_unstable();
    assert_eq!(names, ["media/portrait.jpg", "notes.txt"]);
    assert_eq!(
        reader.read_media_file("notes.txt").unwrap(),
        b"Hello, Sampleton!"
    );
    assert_eq!(
        reader.read_media_file("media/portrait.jpg").unwrap(),
        [0xFF, 0xD8, 0xFF, 0xE0]
    );
}

#[test]
fn an_archive_without_media_is_empty() {
    let bytes = write_gedzip(&Dataset::parse(FAMILY)).unwrap();
    let reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.len(), 1);
    assert!(reader.is_empty());
    assert!(reader.media_files().is_empty());
}

#[test]
fn entries_over_the_limit_are_refused() {
    let bytes = archive(&[("media/big.txt", &[b'a'; 4096])]);
    let mut reader = GedzipReader::new(Cursor::new(bytes))
        .unwrap()
        .max_entry_size(1024);
    assert!(matches!(
        reader.read_media_file("media/big.txt"),
        Err(GedzipError::EntryTooLarge { limit: 1024, .. })
    ));
    // Entries within the limit are still read.
    assert!(reader.read_gedcom_bytes().is_ok());
    assert!(reader.read_dataset(&GedcomBuilder::new()).is_ok());
}

#[test]
fn the_builder_limit_and_strict_mode_apply_to_the_archive() {
    let bytes = write_gedzip(&Dataset::parse(FAMILY)).unwrap();
    // The size limit applies to gedcom.ged as it is decompressed.
    assert!(matches!(
        GedcomBuilder::new()
            .max_file_size(10)
            .build_from_gedzip(Cursor::new(&bytes)),
        Err(GedcomError::Gedzip(GedzipError::EntryTooLarge {
            limit: 10,
            ..
        }))
    ));
    // The reader's own limit and the builder's: the smaller one applies.
    let mut reader = GedzipReader::new(Cursor::new(&bytes))
        .unwrap()
        .max_entry_size(1 << 20);
    assert!(matches!(
        reader.read_dataset(&GedcomBuilder::new().max_file_size(10)),
        Err(GedcomError::Gedzip(GedzipError::EntryTooLarge {
            limit: 10,
            ..
        }))
    ));

    // The writer repairs `SEX male`: the archive holds a conformant file.
    let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n");
    let repaired = write_gedzip(&data).unwrap();
    assert!(
        GedcomBuilder::new()
            .strict(true)
            .build_from_gedzip(Cursor::new(repaired))
            .is_ok()
    );
    // Raw bytes are stored as they are, and strict mode refuses them.
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    writer
        .write_gedcom_bytes(b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n")
        .unwrap();
    let raw = writer.finish().unwrap().into_inner();
    assert!(matches!(
        GedcomBuilder::new()
            .strict(true)
            .build_from_gedzip(Cursor::new(&raw)),
        Err(GedcomError::NonConformant(_))
    ));
    // Leniently, the value is kept.
    let lenient = read_gedzip(&raw).unwrap();
    assert!(
        lenient
            .individuals()
            .get(0)
            .unwrap()
            .into_owned()
            .sex
            .as_ref()
            .is_some_and(|s| !s.is_known())
    );
}

#[test]
fn file_references_find_their_entry() {
    let bytes = archive(&[("Media/Family Photo.JPG", b"JPEG BYTES")]);
    let mut reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
    for reference in [
        "Media/Family Photo.JPG",
        "media/family%20photo.jpg",
        "./Media/Family Photo.JPG",
        "/Media/Family Photo.JPG",
        "Media\\Family Photo.JPG",
    ] {
        assert_eq!(
            reader.find_entry(reference),
            Some("Media/Family Photo.JPG"),
            "{reference}"
        );
        assert!(reader.contains_file(reference), "{reference}");
        assert_eq!(reader.read_media_file(reference).unwrap(), b"JPEG BYTES");
    }
    assert_eq!(reader.find_entry("media/other.jpg"), None);
}

/// A `FILE` payload percent-escapes what a URL string cannot hold; the
/// archive's entry names are not escaped (7.0.17 §4).
#[test]
fn media_added_by_their_file_payload_are_named_unescaped() {
    let bytes = archive(&[("photos/an%20%C3%A9t%C3%A9.jpg", b"JPEG BYTES")]);
    let mut reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.media_files(), ["photos/an \u{e9}t\u{e9}.jpg"]);
    for reference in [
        "photos/an%20%C3%A9t%C3%A9.jpg",
        "photos/an%20\u{e9}t\u{e9}.jpg",
    ] {
        assert_eq!(reader.read_media_file(reference).unwrap(), b"JPEG BYTES");
    }
    // An entry name as it is is stored as it is.
    let bytes = archive(&[("photos/an \u{e9}t\u{e9}.jpg", b"JPEG BYTES")]);
    let reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.media_files(), ["photos/an \u{e9}t\u{e9}.jpg"]);
}

#[test]
fn compressed_media_are_stored_as_they_are() {
    let bytes = archive(&[("photo.jpg", b"JPEG BYTES"), ("notes.txt", b"plain text")]);
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let method = |zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str| {
        let mut entry = zip.by_name(name).unwrap();
        let mut sink = Vec::new();
        entry.read_to_end(&mut sink).unwrap();
        entry.compression()
    };
    assert_eq!(
        method(&mut zip, "photo.jpg"),
        zip::CompressionMethod::Stored
    );
    assert_eq!(
        method(&mut zip, "notes.txt"),
        zip::CompressionMethod::Deflated
    );
    assert_eq!(
        method(&mut zip, GEDCOM_FILENAME),
        zip::CompressionMethod::Deflated
    );
}

#[test]
fn errors() {
    // Not a zip.
    assert!(matches!(
        GedcomBuilder::new().build_from_gedzip(Cursor::new(b"not a zip")),
        Err(GedcomError::Gedzip(GedzipError::Zip(_)))
    ));
    assert!(matches!(
        read_gedzip(b"not a zip"),
        Err(GedcomError::Gedzip(GedzipError::Zip(_)))
    ));

    // A zip without gedcom.ged.
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("other.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"text").unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    assert!(matches!(
        GedzipReader::new(Cursor::new(&bytes)),
        Err(GedzipError::MissingGedcom)
    ));
    assert!(matches!(
        read_gedzip(&bytes),
        Err(GedcomError::Gedzip(GedzipError::MissingGedcom))
    ));

    // A media file the archive does not hold.
    let mut reader = GedzipReader::new(Cursor::new(archive(&[]))).unwrap();
    assert!(matches!(
        reader.read_media_file("missing.jpg"),
        Err(GedzipError::MissingMedia(name)) if name == "missing.jpg"
    ));
}

/// An archive holds GEDCOM 7.x in UTF-8 with a byte order mark: a 5.5.1
/// dataset is converted to 7.0, whatever version the writer is set to but
/// a 7.x one.
#[test]
fn an_archive_holds_gedcom_7() {
    let v551 = Dataset::parse(
        "0 HEAD\n1 GEDC\n2 VERS 5.5.1\n1 CHAR ANSEL\n0 @I1@ INDI\n1 NAME Ann /Example/\n\
         1 OBJE\n2 FILE media\\portrait.jpg\n3 FORM jpg\n2 TITL Portrait\n0 TRLR\n",
    );
    let gedcom = |bytes: Vec<u8>| {
        let mut reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
        reader.read_gedcom_bytes().unwrap()
    };
    let bytes = gedcom(write_gedzip(&v551).unwrap());
    assert!(bytes.starts_with("\u{feff}0 HEAD\n1 GEDC\n2 VERS 7.0\n".as_bytes()));
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("1 OBJE @M1@\n2 TITL Portrait\n"), "{text}");
    assert!(
        text.contains("0 @M1@ OBJE\n1 FILE media/portrait.jpg\n2 FORM image/jpeg\n"),
        "{text}"
    );
    assert!(!text.contains("CHAR"), "{text}");
    assert!(oxidged::spec::validate_text(&text).is_empty(), "{text}");

    let with = |writer: GedcomWriter, data: &Dataset| {
        let mut zip = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
        zip.write_dataset_with(data, &writer).unwrap();
        gedcom(zip.finish().unwrap().into_inner())
    };
    let v551_writer = GedcomWriter::new().gedcom_version(GedcomVersion::V5_5_1);
    assert!(
        with(v551_writer.clone(), &v551)
            .starts_with("\u{feff}0 HEAD\n1 GEDC\n2 VERS 7.0\n".as_bytes())
    );
    let v71 = GedcomWriter::new().gedcom_version(GedcomVersion::V7_1);
    assert!(with(v71, &v551).starts_with("\u{feff}0 HEAD\n1 GEDC\n2 VERS 7.1\n".as_bytes()));
    // A 7.1 dataset stays 7.1.
    let data71 = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.1\n0 TRLR\n");
    assert!(
        with(v551_writer, &data71).starts_with("\u{feff}0 HEAD\n1 GEDC\n2 VERS 7.1\n".as_bytes())
    );
    // A writer may leave the byte order mark out.
    let bare = GedcomWriter::new().bom(oxidged::Bom::Never);
    assert!(with(bare, &v551).starts_with(b"0 HEAD\n"));
}

/// The names GEDZIP reserves are refused as media names (§2.12, §4).
#[test]
fn reserved_names_are_refused() {
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.write_dataset(&Dataset::parse(FAMILY)).unwrap();
    for name in [
        "gedcom.ged",
        "./gedcom.ged",
        "META-INF/container.xml",
        "MANIFEST.MF",
        "media/",
        "",
    ] {
        assert!(
            matches!(writer.add_media_file(name, b"x"), Err(GedzipError::ReservedName(n)) if n == name),
            "{name}"
        );
    }
    // Names that only look alike are media.
    writer.add_media_file("media/gedcom.ged.jpg", b"x").unwrap();
    writer.add_media_file("meta-info/a.jpg", b"x").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(
        reader.media_files(),
        ["media/gedcom.ged.jpg", "meta-info/a.jpg"]
    );
}

/// Directories are no media, and an exact name, then a decoded one, wins
/// over a name in another case.
#[test]
fn directories_are_not_media_and_names_are_case_sensitive() {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file(GEDCOM_FILENAME, options).unwrap();
    zip.write_all(b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n")
        .unwrap();
    zip.add_directory("media/", options).unwrap();
    for name in ["media/Photo 1.jpg", "media/photo 1.jpg"] {
        zip.start_file(name, options).unwrap();
        zip.write_all(name.as_bytes()).unwrap();
    }
    let bytes = zip.finish().unwrap().into_inner();
    let mut reader = GedzipReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(
        reader.media_files(),
        ["media/Photo 1.jpg", "media/photo 1.jpg"]
    );
    assert_eq!(reader.find_entry("media/"), None);
    assert_eq!(
        reader.find_entry("media/photo%201.jpg"),
        Some("media/photo 1.jpg")
    );
    assert_eq!(
        reader.find_entry("media/Photo%201.jpg"),
        Some("media/Photo 1.jpg")
    );
    assert_eq!(
        reader.find_entry("MEDIA/PHOTO%201.JPG"),
        Some("media/Photo 1.jpg")
    );
    assert_eq!(
        reader.read_media_file("media/photo%201.jpg").unwrap(),
        b"media/photo 1.jpg"
    );
}
