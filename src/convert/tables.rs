//! The facts conversion maps by: language names, relation words, file
//! formats, identifier types and the URIs of well-known extension tags.

/// The 5.5.1 language names (`LANGUAGE_ID`, p. 51) and their BCP 47 tags,
/// in the order of the 5.5.1 list: each 5.5.1 name is the name a tag is
/// written back as. The mapping is that of the gedcom7code test files
/// (`lang-all`).
pub(super) const LANGUAGES: &[(&str, &str)] = &[
    ("Afrikaans", "af"),
    ("Albanian", "sq"),
    ("Amharic", "am"),
    ("Anglo-Saxon", "ang"),
    ("Arabic", "ar"),
    ("Armenian", "hy"),
    ("Assamese", "as"),
    ("Belorusian", "be"),
    ("Bengali", "bn"),
    ("Braj", "bra"),
    ("Bulgarian", "bg"),
    ("Burmese", "my"),
    ("Cantonese", "yue"),
    ("Catalan", "ca"),
    ("Catalan_Spn", "ca-ES"),
    ("Church-Slavic", "cu"),
    ("Czech", "cs"),
    ("Danish", "da"),
    ("Dogri", "dgr"),
    ("Dutch", "nl"),
    ("English", "en"),
    ("Esperanto", "eo"),
    ("Estonian", "et"),
    ("Faroese", "fo"),
    ("Finnish", "fi"),
    ("French", "fr"),
    ("Georgian", "ka"),
    ("German", "de"),
    ("Greek", "el"),
    ("Gujarati", "gu"),
    ("Hawaiian", "haw"),
    ("Hebrew", "he"),
    ("Hindi", "hi"),
    ("Hungarian", "hu"),
    ("Icelandic", "is"),
    ("Indonesian", "id"),
    ("Italian", "it"),
    ("Japanese", "ja"),
    ("Kannada", "kn"),
    ("Khmer", "km"),
    ("Konkani", "kok"),
    ("Korean", "ko"),
    ("Lahnda", "lah"),
    ("Lao", "lo"),
    ("Latvian", "lv"),
    ("Lithuanian", "lt"),
    ("Macedonian", "mk"),
    ("Maithili", "mai"),
    ("Malayalam", "ml"),
    ("Mandrin", "cmn"),
    ("Manipuri", "mni"),
    ("Marathi", "mr"),
    ("Mewari", "mtr"),
    ("Navaho", "nv"),
    ("Nepali", "ne"),
    ("Norwegian", "no"),
    ("Oriya", "or"),
    ("Pahari", "him"),
    ("Pali", "pi"),
    ("Panjabi", "pa"),
    ("Persian", "fa"),
    ("Polish", "pl"),
    ("Portuguese", "pt"),
    ("Prakrit", "pra"),
    ("Pusto", "ps"),
    ("Rajasthani", "raj"),
    ("Romanian", "ro"),
    ("Russian", "ru"),
    ("Sanskrit", "sa"),
    ("Serbo_Croa", "sh"),
    ("Serb", "sr"),
    ("Slovak", "sk"),
    ("Slovene", "sl"),
    ("Spanish", "es"),
    ("Swedish", "sv"),
    ("Tagalog", "tl"),
    ("Tamil", "ta"),
    ("Telugu", "te"),
    ("Thai", "th"),
    ("Tibetan", "bo"),
    ("Turkish", "tr"),
    ("Ukrainian", "uk"),
    ("Urdu", "ur"),
    ("Vietnamese", "vi"),
    ("Wendic", "wen"),
    ("Yiddish", "yi"),
];

/// Common spellings of language names outside the 5.5.1 list, read when
/// converting to 7.x (never written back: the 5.5.1 list names them
/// otherwise or not at all).
pub(super) const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("Belarusian", "be"),
    ("Chinese", "zh"),
    ("Croatian", "hr"),
    ("Latin", "la"),
    ("Mandarin", "cmn"),
    ("Navajo", "nv"),
    ("Pashto", "ps"),
    ("Punjabi", "pa"),
    ("Serbian", "sr"),
    ("Slovenian", "sl"),
];

/// The 7.x `ROLE` values a 5.5.1 relation word names (`ASSO.RELA`),
/// matched without regard to case; any other word is `OTHER`.
pub(super) const ROLE_WORDS: &[(&str, &str)] = &[
    ("child", "CHIL"),
    ("kid", "CHIL"),
    ("chil", "CHIL"),
    ("husband", "HUSB"),
    ("hus", "HUSB"),
    ("husb", "HUSB"),
    ("wife", "WIFE"),
    ("mother", "MOTH"),
    ("moth", "MOTH"),
    ("father", "FATH"),
    ("fath", "FATH"),
    ("spouse", "SPOU"),
    ("spou", "SPOU"),
    ("clergy", "CLERGY"),
    ("priest", "CLERGY"),
    ("pastor", "CLERGY"),
    ("minister", "CLERGY"),
    ("friend", "FRIEND"),
    ("godparent", "GODP"),
    ("god parent", "GODP"),
    ("godmother", "GODP"),
    ("god mother", "GODP"),
    ("godfather", "GODP"),
    ("god father", "GODP"),
    ("godp", "GODP"),
    ("neighbor", "NGHBR"),
    ("neighbour", "NGHBR"),
    ("nghbr", "NGHBR"),
    ("officiator", "OFFICIATOR"),
    ("official", "OFFICIATOR"),
    ("parent", "PARENT"),
    ("witness", "WITN"),
    ("witn", "WITN"),
];

/// The relation word each 7.x `ROLE` value is written as in 5.5.1
/// (`ASSO.RELA`) when no `PHRASE` words it.
pub(super) const ROLE_NAMES: &[(&str, &str)] = &[
    ("CHIL", "child"),
    ("HUSB", "husband"),
    ("WIFE", "wife"),
    ("MOTH", "mother"),
    ("FATH", "father"),
    ("SPOU", "spouse"),
    ("CLERGY", "clergy"),
    ("FRIEND", "friend"),
    ("GODP", "godparent"),
    ("NGHBR", "neighbor"),
    ("OFFICIATOR", "officiator"),
    ("PARENT", "parent"),
    ("WITN", "witness"),
    ("OTHER", "other"),
];

/// 5.5.1 file formats (`MULTIMEDIA_FORMAT`, p. 54, and the formats files
/// name in practice) and their media types; the first format of a media
/// type is the one written back. Any other format `f` is the media type
/// `application/x-f`.
pub(super) const MEDIA_TYPES: &[(&str, &str)] = &[
    ("bmp", "image/bmp"),
    ("gif", "image/gif"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("ole", "application/x-oleobject"),
    ("pcx", "image/vnd.zbrush.pcx"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("wav", "audio/wav"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("jxl", "image/jxl"),
    ("svg", "image/svg+xml"),
    ("pdf", "application/pdf"),
    ("txt", "text/plain"),
    ("htm", "text/html"),
    ("html", "text/html"),
    ("mp3", "audio/mpeg"),
    ("ogg", "audio/ogg"),
    ("mp4", "video/mp4"),
    ("mpg", "video/mpeg"),
    ("mpeg", "video/mpeg"),
    ("avi", "video/x-msvideo"),
];

/// The phonetic (`FONE.TYPE`) and romanized (`ROMN.TYPE`) methods of 5.5.1
/// and the BCP 47 tags of their 7.x translations (`TRAN.LANG`), from the
/// `FamilySearch` migration guide; `true` for a phonetic method.
pub(super) const TRANSLATION_METHODS: &[(&str, bool, &str)] = &[
    ("hangul", true, "ko-Hang"),
    ("kana", true, "ja-Hrkt"),
    ("pinyin", false, "und-Latn-pinyin"),
    ("romaji", false, "ja-Latn"),
    ("romanji", false, "ja-Latn"),
    ("wadegiles", false, "zh-Latn-wadegile"),
];

/// The base of the URIs of the `EXID.TYPE`s that stand for 5.5.1
/// identifiers (GEDCOM-registries `exid-types`): `AFN`, `RFN#system` and
/// `RIN#system`.
pub(super) const EXID_TERMS: &str = "https://gedcom.io/terms/v7/";

/// The `EXID.TYPE` of a `FamilySearch` Family Tree person identifier, which
/// 5.5.1 files hold as `_FSFTID` or `_FID`.
pub(super) const FAMILYSEARCH_PERSON: &str = "https://gedcom.io/exid-type/FamilySearch-PersonId";

/// Extension tags 5.5.1 files use whose meaning has a URI, declared in
/// `HEAD.SCHMA` when they are written in 7.x (the GEDCOM-L tags, as the
/// gedcom7code converter declares them).
pub(crate) const KNOWN_EXTENSIONS: &[(&str, &str)] = &[
    ("_AIDN", "http://genealogy.net/GEDCOM#_AIDN"),
    ("_DMGD", "http://genealogy.net/GEDCOM#_DMGD"),
    ("_GOV", "http://genealogy.net/GEDCOM#_GOV"),
    ("_GOVTYPE", "http://genealogy.net/GEDCOM#_GOVTYPE"),
    ("_LOC", "http://genealogy.net/GEDCOM#_LOC"),
    ("_MAIDENHEAD", "http://genealogy.net/GEDCOM#_MAIDENHEAD"),
    ("_POST", "http://genealogy.net/GEDCOM#_POST"),
];

/// The URIs of the 5.5.1 structures 7.x drops, which
/// [`ConversionOptions::keep_obsolete`](super::ConversionOptions::keep_obsolete)
/// writes as extensions (GEDCOM-registries `v5.5.1` terms).
pub(super) const RECORD_SUBN_URI: &str = "https://gedcom.io/terms/v5.5.1/record-SUBN";
/// `HEAD.SUBN`.
pub(super) const HEAD_SUBN_URI: &str = "https://gedcom.io/terms/v5.5.1/SUBN";
/// `HEAD.FILE`.
pub(super) const HEAD_FILE_URI: &str = "https://gedcom.io/terms/v5.5.1/HEAD-FILE";
/// The 7.x `PHRASE`, which a relocated phrase is declared as.
pub(super) const PHRASE_URI: &str = "https://gedcom.io/terms/v7/PHRASE";
