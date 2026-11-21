//! GEDZIP archives: reading and writing a dataset and its media through
//! the builder, the reader and writer types and the convenience functions;
//! entry limits, `FILE` reference matching, compression and errors. Every
//! name and value is fictitious.
#![cfg(feature = "gedzip")]

use std::collections::HashMap;
use std::io::{BufReader, Cursor, Read, Write};
use std::path::Path;

use oxidged::gedzip::{
    GEDCOM_FILENAME, GedzipError, GedzipReader, GedzipWriter, read_gedzip, write_gedzip,
    write_gedzip_with_media,
};
use oxidged::model::Dataset;
use oxidged::profile::Interpretation;
use oxidged::{GedcomBuilder, GedcomError, GedcomStreamParser, GedcomVersion, GedcomWriter};

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

/// `bytes`, an archive, with the uncompressed size its entry `name`
/// declares set to `size`, in its local header and its central directory
/// record: an archive that lies about the entry.
fn declaring(mut bytes: Vec<u8>, name: &str, size: u32) -> Vec<u8> {
    // The signature of each header, then where its uncompressed size, its
    // name's length and its name are.
    for (signature, size_at, len_at, name_at) in
        [(*b"PK\x03\x04", 22, 26, 30), (*b"PK\x01\x02", 24, 28, 46)]
    {
        let mut at = 0;
        while let Some(found) = bytes[at..].windows(4).position(|w| w == signature) {
            let found = found + at;
            let len = usize::from(u16::from_le_bytes([
                bytes[found + len_at],
                bytes[found + len_at + 1],
            ]));
            if bytes.get(found + name_at..found + name_at + len) == Some(name.as_bytes()) {
                bytes[found + size_at..found + size_at + 4].copy_from_slice(&size.to_le_bytes());
            }
            at = found + 4;
        }
    }
    bytes
}

/// A reader that fails after giving `left` bytes.
struct Failing {
    left: usize,
}

impl Read for Failing {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Err(std::io::Error::other("the source failed"));
        }
        let n = buf.len().min(self.left);
        buf[..n].fill(b'x');
        self.left -= n;
        Ok(n)
    }
}

/// Media files are written from a reader and read as a reader, named,
/// stored and compressed as the whole-file functions do.
#[test]
fn media_files_stream_through_an_archive() {
    let text = b"A letter of Ann Example, transcribed.".repeat(100);
    let photo = [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
    let whole = archive(&[
        ("letters/a%20letter.txt", &text),
        ("media/portrait.jpg", &photo),
    ]);

    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    writer
        .write_gedcom_bytes(b"0 HEAD\n1 GEDC\n2 VERS 7.0\n0 TRLR\n")
        .unwrap();
    let size = writer
        .add_media_file_from("letters/a%20letter.txt", &mut text.as_slice())
        .unwrap();
    assert_eq!(size, text.len() as u64);
    let mut file = Cursor::new(photo.to_vec());
    let size = writer
        .add_media_file_from("media/portrait.jpg", &mut file)
        .unwrap();
    assert_eq!(size, 7);
    let streamed = writer.finish().unwrap().into_inner();
    // The same archive, byte for byte.
    assert_eq!(streamed, whole);

    let mut reader = GedzipReader::new(Cursor::new(streamed)).unwrap();
    assert_eq!(
        reader.media_files(),
        ["letters/a letter.txt", "media/portrait.jpg"]
    );
    // The entry is found by its FILE reference, as `read_media_file` finds it.
    let mut entry = reader.open_media_file("Letters/A%20Letter.TXT").unwrap();
    assert_eq!(entry.name(), "letters/a letter.txt");
    assert_eq!(entry.size(), text.len() as u64);
    let mut read = Vec::new();
    // In small reads, as a caller copying it would.
    let mut buf = [0; 7];
    loop {
        let n = entry.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        read.extend_from_slice(&buf[..n]);
    }
    assert_eq!(read, text);
    assert!(format!("{entry:?}").contains("letters/a letter.txt"));
    drop(entry);
    let mut read = Vec::new();
    reader
        .open_media_file("media/portrait.jpg")
        .unwrap()
        .read_to_end(&mut read)
        .unwrap();
    assert_eq!(read, photo);
    assert!(matches!(
        reader.open_media_file("media/missing.jpg"),
        Err(GedzipError::MissingMedia(name)) if name == "media/missing.jpg"
    ));
    // Directories are no media.
    assert!(matches!(
        reader.open_media_file("media/"),
        Err(GedzipError::MissingMedia(_))
    ));
}

/// `gedcom.ged` is written from a reader and read as a reader, which feeds
/// the stream parser.
#[test]
fn the_gedcom_file_streams_through_an_archive() {
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    assert!(!writer.has_gedcom());
    let size = writer
        .write_gedcom_from(&mut Cursor::new(FAMILY.as_bytes()))
        .unwrap();
    assert!(writer.has_gedcom());
    assert_eq!(size, FAMILY.len() as u64);
    let streamed = writer.finish().unwrap().into_inner();
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.write_gedcom_bytes(FAMILY.as_bytes()).unwrap();
    assert_eq!(streamed, writer.finish().unwrap().into_inner());

    let mut reader = GedzipReader::new(Cursor::new(streamed)).unwrap();
    let mut entry = reader.open_gedcom().unwrap();
    assert_eq!(entry.name(), GEDCOM_FILENAME);
    assert_eq!(entry.size(), FAMILY.len() as u64);
    let mut text = String::new();
    entry.read_to_string(&mut text).unwrap();
    assert_eq!(text, FAMILY);
    drop(entry);

    let parser = GedcomStreamParser::new(BufReader::new(reader.open_gedcom().unwrap())).unwrap();
    let streamed: Dataset = parser.collect::<Result<_, _>>().unwrap();
    let read = reader.read_dataset(&GedcomBuilder::new()).unwrap();
    assert_eq!(streamed.to_structures(), read.to_structures());
}

/// The limit set with `max_entry_size` applies to the entries opened as
/// readers, as to those read whole.
#[test]
fn streamed_entries_keep_the_size_limit() {
    let bytes = archive(&[("media/big.txt", &[b'a'; 4096]), ("media/small.txt", b"ok")]);
    let mut reader = GedzipReader::new(Cursor::new(bytes))
        .unwrap()
        .max_entry_size(1024);
    assert!(matches!(
        reader.open_media_file("media/big.txt"),
        Err(GedzipError::EntryTooLarge { name, limit: 1024 }) if name == "media/big.txt"
    ));
    let mut small = Vec::new();
    reader
        .open_media_file("media/small.txt")
        .unwrap()
        .read_to_end(&mut small)
        .unwrap();
    assert_eq!(small, b"ok");
    assert!(reader.open_gedcom().is_ok());

    // gedcom.ged too.
    let bytes = write_gedzip(&Dataset::parse(FAMILY)).unwrap();
    let mut reader = GedzipReader::new(Cursor::new(bytes))
        .unwrap()
        .max_entry_size(10);
    assert!(matches!(
        reader.open_gedcom(),
        Err(GedzipError::EntryTooLarge { limit: 10, .. })
    ));
}

/// An entry that declares less than it holds is refused, streamed or
/// whole, with or without a limit: the archive library stops at the
/// declared size, before the limit can be reached.
#[test]
fn an_entry_that_declares_less_than_it_holds_is_refused() {
    let lying = || {
        declaring(
            archive(&[("media/big.txt", &[b'a'; 4096])]),
            "media/big.txt",
            100,
        )
    };
    let refused = |result: Result<Vec<u8>, GedzipError>| match result {
        Err(GedzipError::Io(e)) => e.kind() == std::io::ErrorKind::InvalidData,
        Err(GedzipError::EntryTooLarge { .. }) => true,
        _ => false,
    };
    for limit in [None, Some(1024), Some(4096)] {
        let reader = || {
            let reader = GedzipReader::new(Cursor::new(lying())).unwrap();
            match limit {
                Some(limit) => reader.max_entry_size(limit),
                None => reader,
            }
        };
        let mut whole = reader();
        assert!(refused(whole.read_media_file("media/big.txt")), "{limit:?}");
        let mut streamed = reader();
        let mut entry = streamed.open_media_file("media/big.txt").unwrap();
        assert_eq!(entry.size(), 100);
        let mut bytes = Vec::new();
        let result = entry
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(GedzipError::from);
        assert!(refused(result), "{limit:?}");
    }
    // Under a limit below the declared size, it is refused before reading.
    let mut reader = GedzipReader::new(Cursor::new(lying()))
        .unwrap()
        .max_entry_size(50);
    assert!(matches!(
        reader.open_media_file("media/big.txt"),
        Err(GedzipError::EntryTooLarge { limit: 50, .. })
    ));
}

/// A media file streamed in takes the names `add_media_file` takes, and the
/// reader is not read for a name refused.
#[test]
fn reserved_names_are_refused_to_streamed_media() {
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
        let mut source = Cursor::new(b"x".to_vec());
        assert!(
            matches!(
                writer.add_media_file_from(name, &mut source),
                Err(GedzipError::ReservedName(n)) if n == name
            ),
            "{name}"
        );
        assert_eq!(source.position(), 0, "{name}");
    }
    let bytes = writer.finish().unwrap().into_inner();
    assert!(
        GedzipReader::new(Cursor::new(bytes))
            .unwrap()
            .media_files()
            .is_empty()
    );
}

/// A source that fails fails the entry, with its I/O error.
#[test]
fn a_failing_source_fails_the_entry() {
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    assert!(matches!(
        writer.add_media_file_from("media/scan.tif", &mut Failing { left: 100_000 }),
        Err(GedzipError::Io(e)) if e.to_string() == "the source failed"
    ));
    let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    assert!(matches!(
        writer.write_gedcom_from(&mut Failing { left: 10 }),
        Err(GedzipError::Io(_))
    ));
    assert!(!writer.has_gedcom());
}

/// Every GEDCOM file of the test fixtures, in every encoding they come in.
fn fixtures() -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|e| e == "ged") {
                let bytes = std::fs::read(&path).unwrap();
                files.push((path.display().to_string(), bytes));
            }
        }
    }
    let mut files = Vec::new();
    walk(Path::new("tests/fixtures"), &mut files);
    assert!(files.len() > 50, "{}", files.len());
    files
}

/// `read_dataset` reads the archive's `gedcom.ged` as the builder reads the
/// file's bytes: the same encoding detection, interpretation and strict
/// mode, the same dataset.
#[test]
fn read_dataset_reads_the_file_as_the_builder_reads_its_bytes() {
    let builders = [
        GedcomBuilder::new(),
        GedcomBuilder::new().interpretation(Interpretation::None),
        GedcomBuilder::new().strict(true),
        GedcomBuilder::new().max_file_size(1 << 20),
    ];
    for (path, bytes) in fixtures() {
        let mut writer = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
        writer.write_gedcom_bytes(&bytes).unwrap();
        let zipped = writer.finish().unwrap().into_inner();
        for builder in &builders {
            let mut reader = GedzipReader::new(Cursor::new(&zipped)).unwrap();
            let from_archive = reader.read_dataset(builder);
            let from_bytes = builder.build_from_bytes(bytes.clone());
            match (from_archive, from_bytes) {
                (Ok(a), Ok(b)) => assert_eq!(a, b, "{path}"),
                (Err(a), Err(b)) => assert_eq!(format!("{a:?}"), format!("{b:?}"), "{path}"),
                (a, b) => panic!("{path}: {:?} against {:?}", a.is_ok(), b.is_ok()),
            }
            assert_eq!(reader.read_gedcom_bytes().unwrap(), bytes, "{path}");
        }
    }
}

/// The entries of an archive: name, compression, CRC-32, size and bytes.
fn entries(archive: &[u8]) -> Vec<(String, zip::CompressionMethod, u32, u64, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut entry = zip.by_index(i).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            let name = entry.name().unwrap().into_owned();
            (
                name,
                entry.compression(),
                entry.crc32(),
                entry.size(),
                bytes,
            )
        })
        .collect()
}

/// A writer that keeps the bytes it is given and counts the writes.
#[derive(Default)]
struct Counted {
    bytes: Vec<u8>,
    writes: usize,
}

impl Write for Counted {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `write_dataset_with` writes the GEDCOM file into the archive as the
/// writer produces it: the archive holds the entries of the archive written
/// from the whole text, and is that archive byte for byte when the writer
/// produces its text in one piece. Deflate's output depends on how its
/// input is cut, so a text produced in blocks deflates to other (equally
/// valid) bytes; it is the same for the same dataset and writer.
#[test]
fn write_dataset_with_writes_the_archive_of_the_whole_text() {
    // A dataset the writer writes whole, and the fixtures, one of which
    // (the largest) it writes in blocks.
    let generated = |version: &str, people: usize| {
        let mut text = format!("0 HEAD\n1 GEDC\n2 VERS {version}\n");
        for i in 1..=people {
            text.push_str(&format!(
                "0 @I{i}@ INDI\n1 NAME Person{i} /Sampleton/\n1 SEX M\n1 BIRT\n2 DATE {} JAN 1850\n2 PLAC Exampleville\n",
                i % 28 + 1
            ));
        }
        text.push_str("0 TRLR\n");
        Dataset::parse(text)
    };
    let mut datasets = vec![("generated".to_string(), generated("7.0", 1500), 0)];
    for (path, bytes) in fixtures() {
        let size = bytes.len();
        datasets.push((
            path,
            GedcomBuilder::new().build_from_bytes(bytes).unwrap(),
            size,
        ));
    }
    let writers = [
        GedcomWriter::new(),
        GedcomWriter::new()
            .gedcom_version(GedcomVersion::V7_1)
            .bom(oxidged::Bom::Never)
            .line_ending(oxidged::LineEnding::CrLf),
    ];
    let (mut whole_texts, mut blocks) = (0, 0);
    for (path, data, size) in &datasets {
        // The largest fixtures are slow to write unoptimised: once is enough.
        let writers = if *size > 64 * 1024 {
            &writers[..1]
        } else {
            &writers[..]
        };
        for writer in writers {
            let streamed = || {
                let mut zip = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
                zip.write_dataset_with(data, writer).unwrap();
                assert!(zip.has_gedcom());
                zip.add_media_file("media/portrait.jpg", b"JPEG BYTES")
                    .unwrap();
                zip.finish().unwrap().into_inner()
            };
            let archive = streamed();
            if path == "generated" {
                assert!(archive == streamed(), "{path}");
            }

            // As it was written: the whole text, then its bytes.
            let version = writer
                .config()
                .version
                .filter(|v| v.is_v7())
                .or_else(|| data.version().is_v7().then(|| data.version()))
                .unwrap_or(GedcomVersion::V7_0);
            let writer = writer.clone().gedcom_version(version);
            let mut text = Counted::default();
            writer.write(&mut text, data).unwrap();
            let mut zip = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
            zip.write_gedcom_bytes(&text.bytes).unwrap();
            zip.add_media_file("media/portrait.jpg", b"JPEG BYTES")
                .unwrap();
            let whole = zip.finish().unwrap().into_inner();

            assert!(entries(&archive) == entries(&whole), "{path}");
            if text.writes == 1 {
                whole_texts += 1;
                assert!(archive == whole, "{path}");
            } else {
                blocks += 1;
            }
        }
    }
    // Both cases were seen.
    assert!(whole_texts > 0 && blocks > 0, "{whole_texts} {blocks}");
}

/// Under `RepairPolicy::Error`, a dataset the writer refuses leaves the
/// archive without `gedcom.ged`, and the archive can be written on.
#[test]
fn a_refused_dataset_leaves_no_gedcom_file() {
    let data = Dataset::parse("0 HEAD\n1 GEDC\n2 VERS 7.0\n0 @I1@ INDI\n1 SEX male\n0 TRLR\n");
    let strict = GedcomWriter::new().on_nonconformant(oxidged::RepairPolicy::Error);
    let mut zip = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    assert!(matches!(
        zip.write_dataset_with(&data, &strict),
        Err(GedzipError::Write(oxidged::WriteError::NonConformant(_)))
    ));
    assert!(!zip.has_gedcom());
    zip.write_dataset(&data).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    assert_eq!(read_gedzip(&bytes).unwrap().individuals().len(), 1);
    // A conformant dataset is written under the policy too.
    let mut zip = GedzipWriter::new(Cursor::new(Vec::new())).unwrap();
    zip.write_dataset_with(&Dataset::parse(FAMILY), &strict)
        .unwrap();
    assert!(zip.has_gedcom());
}
