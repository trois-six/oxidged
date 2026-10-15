//! `oxidged`: inspect, validate and rewrite GEDCOM files.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::process::ExitCode;

use oxidged::convert::ConversionOptions;
use oxidged::model::{Dataset, Event, Individual, RecordRef, Sex, View};
use oxidged::profile::{self, Exporter, Interpretation};
use oxidged::{
    GedcomBuilder, GedcomError, GedcomVersion, GedcomWriter, OutputEncoding, WriteError,
};

const HELP: &str = "\
oxidged - inspect, validate and rewrite GEDCOM files (5.5.1, 7.0, 7.1)

USAGE:
  oxidged <file.ged>
  oxidged --individual <XREF> <file.ged>
  oxidged --individual-lastname <LASTNAME> <file.ged>
  oxidged --individual-firstname <FIRSTNAME> <file.ged>
  oxidged --validate [--validation-level <LEVEL>] <file.ged>
  oxidged --write <VERSION> [--keep-obsolete] [--convert-encoding <ENC>] <file.ged>
  oxidged --sanitize [--convert-encoding <ENC>] <file.ged>
  oxidged --profile <NAME> <file.ged>
  oxidged --detect <file.ged>
  oxidged --list-profiles

OPTIONS:
  -h, --help                         Print this help
  --individual <XREF>                Display one individual (e.g. @I1@)
  --individual-lastname <LASTNAME>   List the individuals whose surname contains LASTNAME
                                     (case-insensitive)
  --individual-firstname <FIRSTNAME> List the individuals whose given names contain
                                     FIRSTNAME (case-insensitive)
  --validate                         Check the file against the GEDCOM specification of the
                                     version it declares (5.5.1, 7.0 or 7.1) and list every
                                     deviation with its line
  --validation-level <LEVEL>         strict: deviations are errors (exit code 2);
                                     lenient (default): they are warnings (exit code 0)
  --write <VERSION>                  Write the file to standard output as conformant GEDCOM
                                     5.5.1, 7.0 or 7.1, and list the repairs on standard error;
                                     a 5.5.1 file is converted to 7.x and back
  --keep-obsolete                    With --write 7.0 or 7.1: keep the 5.5.1 structures 7.x
                                     drops (SUBN, HEAD.FILE, escapes) as declared extensions
  --sanitize                         Read leniently and write the file to standard output as
                                     conformant GEDCOM of its own version, listing the repairs
                                     on standard error
  --convert-encoding <ENC>           With --write or --sanitize (implied): write 5.5.1 bytes in
                                     UTF-8, UTF-16 (little endian), ANSEL or ASCII; 7.x is
                                     always UTF-8
  --profile <NAME>                   Write the file to standard output for an importer (e.g.
                                     geneanet, heredis, ancestry; see --list-profiles), listing
                                     what was folded, lost or repaired on standard error
  --detect                           Print the program that produced the file, as detected from
                                     its header and tags
  --list-profiles                    List the producer (import) and importer (export) profiles
  --interpretation <LEVEL>           How a producer's conventions are read: none, lossless
                                     (default) or best-effort

NOTES:
  Any encoding is read. If both --individual-lastname and --individual-firstname are set,
  the individuals matching both are listed. Written files leave out the credentials a
  file may hold (_PASS, an encrypted or server login _USER).

EXIT CODES:
  0 success, 1 I/O error, 2 validation errors, 3 usage error
";

#[derive(Debug, Default)]
struct Args {
    file: Option<String>,
    individual: Option<String>,
    lastname: Option<String>,
    firstname: Option<String>,
    validate: bool,
    strict: Option<bool>,
    write: Option<GedcomVersion>,
    keep_obsolete: bool,
    sanitize: bool,
    encoding: Option<OutputEncoding>,
    profile: Option<String>,
    detect: bool,
    list_profiles: bool,
    interpretation: Option<Interpretation>,
    help: bool,
}

#[derive(Debug)]
enum CliError {
    Io(io::Error),
    Read(GedcomError),
    Write(WriteError),
    Usage(String),
}

impl CliError {
    fn exit_code(&self) -> u8 {
        match self {
            CliError::Io(_) | CliError::Read(_) | CliError::Write(_) => 1,
            CliError::Usage(_) => 3,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Io(e) => write!(f, "I/O error: {e}"),
            CliError::Read(e) => write!(f, "{e}"),
            CliError::Write(e) => write!(f, "{e}"),
            CliError::Usage(msg) => write!(f, "usage error: {msg}"),
        }
    }
}

impl From<io::Error> for CliError {
    fn from(e: io::Error) -> Self {
        CliError::Io(e)
    }
}

fn usage(msg: impl Into<String>) -> CliError {
    CliError::Usage(msg.into())
}

fn parse_args(command_line: &[String]) -> Result<Args, CliError> {
    let mut args = Args::default();
    let mut it = command_line.iter().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str, what: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| usage(format!("{name} expects {what}")))
        };
        match arg.as_str() {
            "-h" | "--help" => args.help = true,
            "--individual" => args.individual = Some(value(arg, "an XREF")?),
            "--individual-lastname" => args.lastname = Some(value(arg, "a LASTNAME")?),
            "--individual-firstname" => args.firstname = Some(value(arg, "a FIRSTNAME")?),
            "--validate" => args.validate = true,
            "--keep-obsolete" => args.keep_obsolete = true,
            "--sanitize" => args.sanitize = true,
            "--detect" => args.detect = true,
            "--list-profiles" => args.list_profiles = true,
            "--profile" => args.profile = Some(value(arg, "a profile name")?),
            "--convert-encoding" => {
                let v = value(arg, "an encoding")?;
                args.encoding = Some(
                    match v.to_ascii_uppercase().replace(['-', '_'], "").as_str() {
                        "UTF8" => OutputEncoding::Utf8,
                        "UTF16" | "UTF16LE" | "UNICODE" => OutputEncoding::Utf16Le,
                        "ANSEL" => OutputEncoding::Ansel,
                        "ASCII" => OutputEncoding::Ascii,
                        _ => {
                            return Err(usage(format!(
                                "unknown encoding: {v} (expected UTF-8, UTF-16, ANSEL or ASCII)"
                            )));
                        }
                    },
                );
            }
            "--interpretation" => {
                let v = value(arg, "none, lossless or best-effort")?;
                args.interpretation = Some(match v.as_str() {
                    "none" => Interpretation::None,
                    "lossless" => Interpretation::Lossless,
                    "best-effort" => Interpretation::BestEffort,
                    _ => {
                        return Err(usage(format!(
                            "unknown interpretation: {v} (expected none, lossless or best-effort)"
                        )));
                    }
                });
            }
            "--validation-level" => {
                args.strict = Some(match value(arg, "strict or lenient")?.as_str() {
                    "strict" => true,
                    "lenient" => false,
                    other => {
                        return Err(usage(format!(
                            "unknown validation level: {other} (expected strict or lenient)"
                        )));
                    }
                });
            }
            "--write" => {
                args.write = Some(match value(arg, "a version")?.as_str() {
                    "5.5.1" => GedcomVersion::V5_5_1,
                    "7.0" => GedcomVersion::V7_0,
                    "7.1" => GedcomVersion::V7_1,
                    other => {
                        return Err(usage(format!(
                            "unknown version: {other} (expected 5.5.1, 7.0 or 7.1)"
                        )));
                    }
                });
            }
            other if other.starts_with('-') => {
                return Err(usage(format!("unknown option: {other}")));
            }
            file => {
                if args.file.is_some() {
                    return Err(usage(
                        "expected one file (quote a path that contains spaces)",
                    ));
                }
                args.file = Some(file.to_string());
            }
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let command_line: Vec<String> = std::env::args().collect();
    match run(&command_line) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(e.exit_code())
        }
    }
}

fn run(command_line: &[String]) -> Result<ExitCode, CliError> {
    let args = parse_args(command_line)?;
    if args.help {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.list_profiles {
        print!("{}", list_profiles());
        return Ok(ExitCode::SUCCESS);
    }
    let file = args
        .file
        .as_deref()
        .ok_or_else(|| usage("missing file (see --help)"))?;
    let filters = args.individual.is_some() || args.lastname.is_some() || args.firstname.is_some();
    if args.strict.is_some() && !args.validate {
        return Err(usage("--validation-level requires --validate"));
    }
    if args.keep_obsolete && args.write.is_none() {
        return Err(usage("--keep-obsolete requires --write"));
    }
    let writes = args.write.is_some() || args.sanitize || args.encoding.is_some();
    if [
        args.validate,
        filters,
        writes,
        args.profile.is_some(),
        args.detect,
    ]
    .iter()
    .filter(|&&b| b)
    .count()
        > 1
    {
        return Err(usage(
            "--validate, --write/--sanitize, --profile, --detect and the --individual options \
             cannot be combined",
        ));
    }
    let export = match &args.profile {
        Some(name) => Some(
            Exporter::named(name)
                .ok_or_else(|| usage(format!("unknown profile: {name} (see --list-profiles)")))?,
        ),
        None => None,
    };
    let bytes = fs::read(file)?;
    if args.validate {
        return Ok(validate(&bytes, args.strict.unwrap_or(false)));
    }
    if args.detect {
        print!("{}", detect(&bytes));
        return Ok(ExitCode::SUCCESS);
    }
    let data = GedcomBuilder::new()
        .interpretation(args.interpretation.unwrap_or_default())
        .build_from_bytes(bytes)
        .map_err(CliError::Read)?;
    let mut out = io::stdout().lock();
    if let Some(exporter) = export {
        let report = exporter.write(&mut out, &data).map_err(CliError::Write)?;
        for fold in report.lossy() {
            eprintln!("lossy: {fold}");
        }
        for limit in &report.limits {
            eprintln!("limit: {limit}");
        }
        for repair in &report.repairs {
            eprintln!("repair: {repair}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    if writes {
        let version = args.write.unwrap_or_else(|| data.version());
        let encoding = args.encoding.unwrap_or_default();
        if version.is_v7() && encoding != OutputEncoding::Utf8 {
            return Err(usage(format!(
                "GEDCOM {version} is always UTF-8: --convert-encoding {encoding} needs 5.5.1"
            )));
        }
        let conversion = ConversionOptions::default().keep_obsolete(args.keep_obsolete);
        let report = GedcomWriter::new()
            .gedcom_version(version)
            .conversion(conversion)
            .output_encoding(encoding)
            .write(&mut out, &data)
            .map_err(CliError::Write)?;
        for repair in &report.repairs {
            eprintln!("repair: {repair}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let text = if let Some(xref) = &args.individual {
        let individual = data
            .find_individual(xref.as_str())
            .ok_or_else(|| usage(format!("no individual {xref}")))?;
        describe(&data, individual)
    } else if filters {
        let wanted = |filter: &Option<String>, pieces: Vec<String>| {
            filter.as_ref().is_none_or(|f| {
                let f = f.to_lowercase();
                pieces.iter().any(|p| p.to_lowercase().contains(&f))
            })
        };
        let mut text = String::new();
        for individual in data.individuals() {
            let (given, surname) = name_parts(individual);
            if wanted(&args.lastname, surname) && wanted(&args.firstname, given) {
                text.push_str(&describe(&data, individual));
            }
        }
        text
    } else {
        summary(&data)
    };
    out.write_all(text.as_bytes())?;
    Ok(ExitCode::SUCCESS)
}

/// The program that produced the file: the detection, its evidence and
/// hints.
fn detect(bytes: &[u8]) -> String {
    let d = profile::detect(bytes);
    let mut text = String::new();
    let _ = writeln!(
        text,
        "producer: {} ({})",
        d.profile().id(),
        d.profile().label()
    );
    if let Some(v) = d.producer_version() {
        let _ = writeln!(text, "version: {v}");
    }
    let _ = writeln!(
        text,
        "score: {}{}",
        d.score(),
        if d.is_known() {
            ""
        } else {
            " (not recognised)"
        }
    );
    for e in d.evidence() {
        let _ = writeln!(text, "  evidence: {e}");
    }
    for h in d.hints() {
        let _ = writeln!(text, "hint: {} ({})", h.id(), h.label());
    }
    text
}

/// The import and export profiles, with their tier.
fn list_profiles() -> String {
    let mut text = String::from("Producers (read):\n");
    for p in profile::import_profiles() {
        let _ = writeln!(text, "  {:<20} tier {}  {}", p.id(), p.tier(), p.label());
    }
    text.push_str("Importers (--profile):\n");
    for p in profile::export_profiles() {
        let _ = writeln!(
            text,
            "  {:<20} tier {}  {}, GEDCOM {}",
            p.id(),
            p.tier(),
            p.label(),
            p.target().version()
        );
    }
    text
}

/// Every deviation of the file from its specification, as errors in strict
/// mode, as warnings otherwise.
fn validate(bytes: &[u8], strict: bool) -> ExitCode {
    let deviations = oxidged::spec::validate_bytes(bytes);
    let (level, kind) = if strict {
        ("strict", "error")
    } else {
        ("lenient", "warning")
    };
    let (errors, warnings) = if strict {
        (deviations.len(), 0)
    } else {
        (0, deviations.len())
    };
    println!("Validation: {level} - errors: {errors}, warnings: {warnings}");
    for d in &deviations {
        println!("{kind}: {d}");
    }
    if errors > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

/// The version, encoding and records of a dataset.
fn summary(data: &Dataset) -> String {
    let mut text = String::new();
    let declared = data
        .declared_version()
        .map(|d| format!(" (declared {d})"))
        .unwrap_or_default();
    let _ = writeln!(text, "GEDCOM {}{declared}", data.version());
    for (name, count) in [
        ("individuals", data.individuals().len()),
        ("families", data.families().len()),
        ("sources", data.sources().len()),
        ("repositories", data.repositories().len()),
        ("multimedia objects", data.multimedia().len()),
        ("shared notes", data.notes().len()),
        ("submitters", data.submitters().len()),
        ("submissions", data.submissions().len()),
        ("other records", data.extra().len()),
    ] {
        let _ = writeln!(text, "  {name}: {count}");
    }
    let dangling = data.dangling_references().len();
    if dangling > 0 {
        let _ = writeln!(text, "  pointers to no record: {dangling}");
    }
    text
}

/// The given names and surnames of an individual's names: their pieces,
/// or the name's words, its surname between slashes.
fn name_parts(individual: View<'_, Individual>) -> (Vec<String>, Vec<String>) {
    let mut given = Vec::new();
    let mut surname = Vec::new();
    for name in individual.names() {
        given.extend(name.givens().map(|g| g.to_string()));
        surname.extend(name.surnames().map(|s| s.to_string()));
        let value = name.value().to_str();
        if let Some(s) = name.surname_in_value() {
            surname.push(s);
        }
        let before = value.split('/').next().unwrap_or_default().trim();
        if !before.is_empty() {
            given.push(before.to_string());
        }
    }
    (given, surname)
}

/// An event's kind, date and place.
fn event_line(label: &str, event: View<'_, Event>) -> String {
    let mut line = format!("  {label}:");
    if let Some(date) = event.date() {
        let _ = write!(line, " {}", date.value());
    }
    if let Some(place) = event.place() {
        let _ = write!(line, ", {}", place.name());
    }
    line.push('\n');
    line
}

/// An individual: names, sex, birth and death, families.
fn describe(data: &Dataset, individual: View<'_, Individual>) -> String {
    let mut text = String::new();
    let xref = individual
        .xref()
        .map_or("(no identifier)", |x| data.store().xref(x));
    let name = individual
        .full_name()
        .unwrap_or_else(|| "(no name)".to_string());
    let _ = writeln!(text, "{xref} {name}");
    for other in individual.names().iter().skip(1) {
        let _ = writeln!(text, "  also: {}", other.full());
    }
    if let Some(sex) = individual.sex() {
        let sex = match sex {
            Sex::Male => "male",
            Sex::Female => "female",
            _ => "other or unknown",
        };
        let _ = writeln!(text, "  sex: {sex}");
    }
    if let Some(birth) = individual.birth() {
        text.push_str(&event_line("born", birth));
    }
    if let Some(death) = individual.death() {
        text.push_str(&event_line("died", death));
    }
    for family in data.families_as_child(individual.xref()) {
        let parents: Vec<String> = data.parents(family).filter_map(|p| p.full_name()).collect();
        if !parents.is_empty() {
            let _ = writeln!(text, "  child of: {}", parents.join(" and "));
        }
    }
    for family in data.families_as_spouse(individual.xref()) {
        if let Some(spouse) = data.spouse(individual.xref(), family) {
            let _ = writeln!(
                text,
                "  partner: {}",
                spouse.full_name().unwrap_or_default()
            );
        }
        for child in data.children(family) {
            let _ = writeln!(text, "  child: {}", child.full_name().unwrap_or_default());
        }
    }
    let records = data
        .records()
        .filter(|r| matches!(r, RecordRef::Individual(i) if i.xref() == individual.xref()))
        .count();
    if records > 1 {
        let _ = writeln!(text, "  ({records} records have this identifier)");
    }
    text
}
