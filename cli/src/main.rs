//! `oxidged`: read any GEDCOM file, write conformant GEDCOM 5.5.1, 7.0 and
//! 7.1, and everything in between, from the command line.

mod edit;
mod io;
mod report;
mod values;

use std::fmt::Write as _;
use std::process::ExitCode;

use clap::{Args, CommandFactory, Parser, Subcommand};
use oxidged::model::{Dataset, RecordRef, XrefId};
use oxidged::profile::{ExportOptions, Exporter, HeadSource};
use oxidged::tree::Tree;
use oxidged::writer::Credentials;
use oxidged::{GedcomError, GedcomStreamParser, GedcomStreamWriter, WriteError, WriterConfig};
use serde_json::json;

use crate::io::{Format, ReadArgs, WriteArgs};
use crate::report::Out;

/// Read any GEDCOM file, write conformant GEDCOM 5.5.1, 7.0 and 7.1.
///
/// Every command that reads a file reads GEDCOM in any encoding, GEDZIP and
/// JSON, from a file or from standard input (`-`); every command that
/// writes one writes GEDCOM, GEDZIP or JSON, to a file or to standard
/// output. Exit codes: 0 success, 1 an input, output or edit error, 2 a
/// deviation from the specification (`validate`, `--strict`, `--no-repair`),
/// 3 a usage error.
#[derive(Parser, Debug)]
#[command(
    name = "oxidged",
    version,
    propagate_version = true,
    max_term_width = 100
)]
struct Cli {
    /// Print reports as JSON (on standard output for the commands that
    /// report, on standard error for those that write a file)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// The version, encoding, producer, records and dangling pointers of a
    /// file
    Info {
        #[command(flatten)]
        read: ReadArgs,
        /// Also list each rule the producer's interpretation applied
        #[arg(long)]
        mappings: bool,
    },
    /// Every deviation of a file from the specification of the version it
    /// declares, with its line (exit code 2 when there is one)
    Validate {
        #[command(flatten)]
        read: ReadArgs,
    },
    /// Write a file as conformant GEDCOM, GEDZIP or JSON: convert between
    /// versions, re-encode, repair
    Write {
        #[command(flatten)]
        read: ReadArgs,
        #[command(flatten)]
        write: WriteArgs,
        /// Read and write GEDCOM one record at a time, in memory bounded by
        /// the largest record
        #[arg(long)]
        stream: bool,
    },
    /// Write a file for an importer (see `oxidged profiles`), reporting what
    /// was folded, lost or repaired
    Export(ExportArgs),
    /// The program that produced a file, with the evidence and hints
    Detect {
        /// The file to read; `-` for standard input
        #[arg(value_name = "FILE")]
        input: String,
    },
    /// The producer and importer profiles, or one of them in detail
    Profiles {
        /// A profile's identifier (e.g. heredis)
        id: Option<String>,
    },
    /// Records: an individual or a family described, any record as GEDCOM
    Show {
        #[command(flatten)]
        read: ReadArgs,
        /// The identifiers of the records (e.g. @I1@)
        #[arg(required = true, value_name = "XREF")]
        xrefs: Vec<String>,
        /// Print the records as GEDCOM lines
        #[arg(long)]
        gedcom: bool,
    },
    /// The individuals whose names match (case-insensitive)
    Search {
        #[command(flatten)]
        read: ReadArgs,
        /// Text one of the full names contains
        query: Option<String>,
        /// Text a surname contains
        #[arg(long)]
        surname: Option<String>,
        /// Text a given name contains
        #[arg(long)]
        given: Option<String>,
    },
    /// The ancestors of an individual, generation by generation
    Ancestors(Relatives),
    /// The descendants of an individual, generation by generation
    Descendants(Relatives),
    /// Write the part of a file some individuals need: them, their
    /// families, and the records they point to
    Subset {
        #[command(flatten)]
        read: ReadArgs,
        /// An individual to keep
        #[arg(long = "individual", short = 'i', value_name = "XREF")]
        individuals: Vec<String>,
        /// An individual to keep with their ancestors
        #[arg(long, value_name = "XREF")]
        ancestors_of: Vec<String>,
        /// An individual to keep with their descendants
        #[arg(long, value_name = "XREF")]
        descendants_of: Vec<String>,
        /// How many generations of ancestors and descendants [default: all]
        #[arg(long, value_name = "N")]
        generations: Option<u32>,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Edit a file, its references kept whole, and write it
    #[command(subcommand)]
    Edit(edit::EditCommand),
    /// The lossless tree of a file: its structures as read, nothing typed
    Tree {
        #[command(flatten)]
        read: ReadArgs,
    },
    /// The encoding of a file: the one its bytes are decoded with, and the
    /// one it declares
    Encoding {
        /// The file to read; `-` for standard input
        #[arg(value_name = "FILE")]
        input: String,
    },
    /// A file's text decoded to UTF-8, as oxidged reads it
    Decode {
        /// The file to read; `-` for standard input
        #[arg(value_name = "FILE")]
        input: String,
        /// Decode in this encoding, whatever the bytes declare
        #[arg(long, value_name = "ENCODING")]
        encoding: Option<String>,
        /// The file to write; standard output by default
        #[arg(short, long, value_name = "FILE")]
        output: Option<String>,
    },
    /// A date payload: how it reads, whether each version accepts it, how
    /// each writes it, and its dates in other calendars
    Date {
        /// The payload, e.g. "ABT @#DJULIAN@ 1700" or "BET 1900 AND 1910"
        value: String,
        /// A calendar to convert to: gregorian, julian, hebrew,
        /// french-republican [default: all]
        #[arg(long = "calendar", value_name = "CALENDAR")]
        calendars: Vec<String>,
    },
    /// An age payload: how it reads, whether each version accepts it and how
    /// each writes it
    Age {
        /// The payload, e.g. "> 2y 3m"
        value: String,
    },
    /// A shell completion script
    Completions {
        /// The shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// The manual pages (roff): `oxidged.1` on standard output, or one per
    /// command in a folder
    Man {
        /// Write `oxidged.1` and a page per command into this folder
        #[arg(long, value_name = "DIR")]
        out_dir: Option<std::path::PathBuf>,
    },
}

#[derive(Args, Debug)]
struct Relatives {
    #[command(flatten)]
    read: ReadArgs,
    /// The individual (e.g. @I1@)
    xref: String,
    /// How many generations [default: all]
    #[arg(long, value_name = "N")]
    generations: Option<u32>,
}

#[derive(Args, Debug)]
struct ExportArgs {
    #[command(flatten)]
    read: ReadArgs,
    /// The importer (see `oxidged profiles`)
    #[arg(long, short = 'p', value_name = "PROFILE")]
    profile: String,
    /// The file to write; `-` or nothing for standard output
    #[arg(short, long, value_name = "FILE")]
    output: Option<String>,
    /// Write this version instead of the profile's
    #[arg(
        short = 'g',
        long = "gedcom-version",
        value_enum,
        value_name = "VERSION"
    )]
    gedcom_version: Option<io::VersionArg>,
    /// Leave out multimedia links and records
    #[arg(long)]
    omit_media: bool,
    /// Leave out the records whose RESN says confidential or privacy, and the
    /// pointers to them
    #[arg(long)]
    omit_private: bool,
    /// Leave out the change dates (CHAN)
    #[arg(long)]
    omit_change_dates: bool,
    /// The individual to write first (the home person of some importers)
    #[arg(long, value_name = "XREF")]
    home: Option<String>,
    /// The folder media paths are made relative to
    #[arg(long, value_name = "PATH")]
    media_root: Option<String>,
    /// Write the credentials a file may hold (left out by default)
    #[arg(long)]
    credentials: bool,
    /// Which system HEAD.SOUR names
    #[arg(long, value_enum, default_value_t, value_name = "WHICH")]
    head_source: HeadSourceArg,
}

/// Which system an export's `HEAD.SOUR` names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
enum HeadSourceArg {
    /// The dataset's when a program set it, else oxidged
    #[default]
    Auto,
    /// oxidged, always
    Library,
    /// The dataset's, whatever it is
    Dataset,
}

/// Why a command failed.
#[derive(Debug)]
pub enum CliError {
    Io(std::io::Error),
    Read(GedcomError),
    Input(String),
    Write(WriteError),
    Output(String),
    Edit(String),
    /// A deviation from the specification (exit code 2).
    Deviations(String),
    Usage(String),
}

impl CliError {
    fn exit_code(&self) -> u8 {
        match self {
            CliError::Read(GedcomError::NonConformant(_))
            | CliError::Write(WriteError::NonConformant(_))
            | CliError::Deviations(_) => 2,
            CliError::Usage(_) => 3,
            _ => 1,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Io(e) => write!(f, "{e}"),
            CliError::Read(GedcomError::NonConformant(deviations)) => {
                writeln!(f, "the input does not conform to its specification:")?;
                for d in deviations {
                    writeln!(f, "  {d}")?;
                }
                Ok(())
            }
            CliError::Read(e) => write!(f, "{e}"),
            CliError::Write(e) => write!(f, "{e}"),
            CliError::Input(e) | CliError::Output(e) | CliError::Deviations(e) => f.write_str(e),
            CliError::Edit(e) => write!(f, "edit refused: {e}"),
            CliError::Usage(e) => write!(f, "usage error: {e}"),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Io(e)
    }
}

impl From<GedcomError> for CliError {
    fn from(e: GedcomError) -> Self {
        CliError::Read(e)
    }
}

impl From<WriteError> for CliError {
    fn from(e: WriteError) -> Self {
        CliError::Write(e)
    }
}

pub fn usage(msg: impl Into<String>) -> CliError {
    CliError::Usage(msg.into())
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return match e.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    ExitCode::SUCCESS
                }
                _ => ExitCode::from(3),
            };
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        // The reader of the output went away (`| head`): nothing to say.
        Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", e.to_string().trim_end());
            ExitCode::from(e.exit_code())
        }
    }
}

fn run(cli: Cli) -> Result<(), CliError> {
    let json = cli.json;
    match cli.command {
        Command::Info { read, mappings } => {
            report::info(&read.load(true)?, mappings).print(json)?;
        }
        Command::Validate { read } => {
            let (bytes, format) = read.bytes()?;
            let gedcom = match format {
                Format::Ged => bytes,
                Format::Gdz => oxidged::gedzip::GedzipReader::new(std::io::Cursor::new(bytes))
                    .and_then(|mut a| a.read_gedcom_bytes())
                    .map_err(|e| CliError::Input(format!("GEDZIP: {e}")))?,
                Format::Json => return Err(usage("validate reads GEDCOM or GEDZIP")),
            };
            let deviations = match &read.encoding {
                Some(label) => oxidged::spec::validate_text(&oxidged::encoding::decode_as(
                    &gedcom,
                    io::encoding_named(label)?,
                )),
                None => oxidged::spec::validate_bytes(&gedcom),
            };
            report::validate(&deviations).print(json)?;
            if !deviations.is_empty() {
                return Err(CliError::Deviations(format!(
                    "{} deviation(s) from the specification",
                    deviations.len()
                )));
            }
        }
        Command::Write {
            read,
            write,
            stream,
        } => {
            if stream {
                return write_stream(&read, &write, json);
            }
            let loaded = read.load(false)?;
            write.write(&loaded.data, &loaded.media, json)?;
        }
        Command::Export(args) => export(args, json)?,
        Command::Detect { input } => {
            let bytes = io::read_bytes(&input)?;
            report::detect(&oxidged::profile::detect(&bytes)).print(json)?;
        }
        Command::Profiles { id } => {
            let out = report::profiles(id.as_deref()).ok_or_else(|| {
                usage(format!(
                    "unknown profile: {} (see `oxidged profiles`)",
                    id.unwrap_or_default()
                ))
            })?;
            out.print(json)?;
        }
        Command::Show {
            read,
            xrefs,
            gedcom,
        } => show(&read.load(false)?.data, &xrefs, gedcom)?.print(json)?,
        Command::Search {
            read,
            query,
            surname,
            given,
        } => {
            if query.is_none() && surname.is_none() && given.is_none() {
                return Err(usage("search needs a QUERY, --surname or --given"));
            }
            let data = read.load(false)?.data;
            search(
                &data,
                query.as_deref(),
                surname.as_deref(),
                given.as_deref(),
            )
            .print(json)?;
        }
        Command::Ancestors(r) => {
            let data = r.read.load(false)?.data;
            individual(&data, &r.xref)?;
            report::relatives(&data, &data.ancestors(r.xref.as_str(), r.generations))
                .print(json)?;
        }
        Command::Descendants(r) => {
            let data = r.read.load(false)?.data;
            individual(&data, &r.xref)?;
            report::relatives(&data, &data.descendants(r.xref.as_str(), r.generations))
                .print(json)?;
        }
        Command::Subset {
            read,
            individuals,
            ancestors_of,
            descendants_of,
            generations,
            write,
        } => {
            if individuals.is_empty() && ancestors_of.is_empty() && descendants_of.is_empty() {
                return Err(usage(
                    "subset needs --individual, --ancestors-of or --descendants-of",
                ));
            }
            let loaded = read.load(false)?;
            let data = &loaded.data;
            let mut chosen: Vec<XrefId> = Vec::new();
            for x in individuals
                .iter()
                .chain(&ancestors_of)
                .chain(&descendants_of)
            {
                chosen.extend(individual(data, x)?);
            }
            for x in &ancestors_of {
                chosen.extend(
                    data.ancestors(x.as_str(), generations)
                        .iter()
                        .filter_map(|r| r.individual.xref()),
                );
            }
            for x in &descendants_of {
                chosen.extend(
                    data.descendants(x.as_str(), generations)
                        .iter()
                        .filter_map(|r| r.individual.xref()),
                );
            }
            let subset = data.subset(chosen);
            write.write(&subset, &loaded.media, json)?;
        }
        Command::Edit(command) => edit::run(command, json)?,
        Command::Tree { read } => tree(&read)?.print(json)?,
        Command::Encoding { input } => encoding(&io::read_bytes(&input)?).print(json)?,
        Command::Decode {
            input,
            encoding,
            output,
        } => {
            let bytes = io::read_bytes(&input)?;
            let (text, used) = match encoding {
                Some(label) => {
                    let e = io::encoding_named(&label)?;
                    (oxidged::encoding::decode_as(&bytes, e), e)
                }
                None => {
                    let d = oxidged::encoding::decode(&bytes);
                    (d.text, d.encoding)
                }
            };
            eprintln!("decoded as {}", used);
            io::write_output(output.as_deref(), text.as_bytes())?;
        }
        Command::Date { value, calendars } => {
            let calendars = if calendars.is_empty() {
                ["gregorian", "julian", "hebrew", "french-republican"]
                    .iter()
                    .filter_map(|c| values::calendar_named(c))
                    .collect()
            } else {
                calendars
                    .iter()
                    .map(|c| {
                        values::calendar_named(c).ok_or_else(|| {
                            usage(format!(
                                "unknown calendar: {c} (expected gregorian, julian, hebrew or \
                                 french-republican)"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            };
            values::date(&value, &calendars).print(json)?;
        }
        Command::Age { value } => values::age(&value).print(json)?,
        Command::Completions { shell } => {
            let mut out = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "oxidged", &mut out);
            io::write_output(None, &out)?;
        }
        Command::Man { out_dir } => man(out_dir.as_deref())?,
    }
    Ok(())
}

/// The identifier of an individual's record.
fn individual(data: &Dataset, xref: &str) -> Result<Option<XrefId>, CliError> {
    data.find_individual(xref)
        .map(|i| i.xref())
        .ok_or_else(|| CliError::Input(format!("no individual {xref}")))
}

/// Records, described or as GEDCOM.
fn show(data: &Dataset, xrefs: &[String], gedcom: bool) -> Result<Out, CliError> {
    let mut text = String::new();
    let mut items = Vec::new();
    for xref in xrefs {
        let record = data
            .find(xref.as_str())
            .ok_or_else(|| CliError::Input(format!("no record {xref}")))?;
        let lines = report::gedcom_of(record, data.version());
        match record {
            RecordRef::Individual(i) if !gedcom => {
                let (t, j) = report::describe(data, i);
                text.push_str(&t);
                items.push(j);
            }
            RecordRef::Family(f) if !gedcom => {
                let _ = writeln!(text, "{xref} family");
                let partners: Vec<_> = data.parents(f).collect();
                for p in &partners {
                    let _ = writeln!(text, "  partner: {}", report::label(data, *p));
                }
                let children: Vec<_> = data.children(f).collect();
                for c in &children {
                    let _ = writeln!(text, "  child: {}", report::label(data, *c));
                }
                items.push(json!({
                    "xref": xref,
                    "type": "family",
                    "partners": partners.iter().map(|p| report::individual_json(data, *p)).collect::<Vec<_>>(),
                    "children": children.iter().map(|c| report::individual_json(data, *c)).collect::<Vec<_>>(),
                    "gedcom": lines,
                }));
            }
            _ => {
                text.push_str(&lines);
                items.push(json!({
                    "xref": xref,
                    "type": report::kind_name(record),
                    "gedcom": lines,
                }));
            }
        }
    }
    Ok(Out {
        text,
        json: serde_json::Value::Array(items),
    })
}

/// The individuals whose names match every filter given.
fn search(data: &Dataset, query: Option<&str>, surname: Option<&str>, given: Option<&str>) -> Out {
    let contains = |filter: Option<&str>, pieces: &[String]| {
        filter.is_none_or(|f| {
            let f = f.to_lowercase();
            pieces.iter().any(|p| p.to_lowercase().contains(&f))
        })
    };
    let query = query.map(str::to_lowercase);
    let mut text = String::new();
    let mut items = Vec::new();
    for individual in data.individuals() {
        let (givens, surnames) = report::name_parts(individual);
        let full = query.as_ref().is_none_or(|q| {
            individual
                .names()
                .into_iter()
                .any(|n| n.full().to_lowercase().contains(q.as_str()))
        });
        if full && contains(surname, &surnames) && contains(given, &givens) {
            let _ = writeln!(text, "{}", report::label(data, individual));
            items.push(report::individual_json(data, individual));
        }
    }
    Out {
        text,
        json: serde_json::Value::Array(items),
    }
}

/// Writes for an importer.
fn export(args: ExportArgs, json: bool) -> Result<(), CliError> {
    let mut exporter = Exporter::named(&args.profile).ok_or_else(|| {
        usage(format!(
            "unknown importer: {} (see `oxidged profiles`)",
            args.profile
        ))
    })?;
    let options = ExportOptions {
        version: args.gedcom_version.map(Into::into),
        omit_media: args.omit_media,
        omit_private: args.omit_private,
        omit_change_dates: args.omit_change_dates,
        home: args.home,
        media_root: args.media_root,
        credentials: if args.credentials {
            Credentials::Written
        } else {
            Credentials::LeftOut
        },
        source: match args.head_source {
            HeadSourceArg::Auto => HeadSource::Auto,
            HeadSourceArg::Library => HeadSource::Library,
            HeadSourceArg::Dataset => HeadSource::Dataset,
        },
    };
    exporter = exporter.options(options);
    let data = args.read.load(false)?.data;
    let mut bytes = Vec::new();
    let report = exporter.write(&mut bytes, &data)?;
    io::write_output(args.output.as_deref(), &bytes)?;
    if json {
        eprintln!(
            "{}",
            json!({
                "folds": report.folds.iter().map(|f| json!({
                    "kind": format!("{:?}", f.kind),
                    "lossy": f.lossy,
                    "record": f.record.as_ref().map(|r| r.as_str().to_string()),
                    "detail": f.detail,
                    "rows": f.rows,
                })).collect::<Vec<_>>(),
                "limits": report.limits.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "repairs": report.repairs.iter().map(|r| json!({
                    "kind": format!("{:?}", r.kind),
                    "detail": r.detail.to_string(),
                })).collect::<Vec<_>>(),
            })
        );
    } else {
        for fold in report.lossy() {
            eprintln!("lossy: {fold}");
        }
        for limit in &report.limits {
            eprintln!("limit: {limit}");
        }
        for repair in &report.repairs {
            eprintln!("repair: {repair}");
        }
    }
    Ok(())
}

/// Rewrites GEDCOM one record at a time.
fn write_stream(read: &ReadArgs, write: &WriteArgs, json: bool) -> Result<(), CliError> {
    if read.from.is_some_and(|f| f != Format::Ged)
        || write.format() != Format::Ged
        || read.encoding.is_some()
        || read.strict
        || read.producer.is_some()
        || write.media_dir.is_some()
    {
        return Err(usage(
            "--stream reads and writes GEDCOM, without --encoding, --strict or --producer",
        ));
    }
    let input: Box<dyn std::io::Read> = if read.input == "-" {
        Box::new(std::io::stdin().lock())
    } else {
        Box::new(std::fs::File::open(&read.input).map_err(|e| {
            CliError::Io(std::io::Error::new(
                e.kind(),
                format!("{}: {e}", read.input),
            ))
        })?)
    };
    let output: Box<dyn std::io::Write> = match write.output.as_deref() {
        None | Some("-") => Box::new(std::io::stdout().lock()),
        Some(path) => Box::new(
            std::fs::File::create(path)
                .map_err(|e| CliError::Io(std::io::Error::new(e.kind(), format!("{path}: {e}"))))?,
        ),
    };
    let output = std::io::BufWriter::new(output);
    let mut records = GedcomStreamParser::new(std::io::BufReader::new(input))?;
    let config: WriterConfig = write
        .writer(&Dataset::new(records.version()))?
        .config()
        .clone();
    let Some(first) = records.next().transpose()? else {
        return Err(CliError::Input("an empty input".into()));
    };
    let mut writer = GedcomStreamWriter::for_dataset(output, config, first.dataset())?;
    if !matches!(first.record(), RecordRef::Header(_)) {
        writer.write_record(first.record())?;
    }
    for record in records {
        writer.write_record(record?.record())?;
    }
    let (mut output, report) = writer.finish()?;
    std::io::Write::flush(&mut output)?;
    io::report_repairs(&report, json);
    Ok(())
}

/// The lossless tree of the input.
fn tree(read: &ReadArgs) -> Result<Out, CliError> {
    let (bytes, format) = read.bytes()?;
    let tree = match format {
        Format::Ged => match &read.encoding {
            Some(label) => Tree::parse(oxidged::encoding::decode_as(
                &bytes,
                io::encoding_named(label)?,
            )),
            None => Tree::from_bytes(bytes),
        },
        Format::Gdz => Tree::from_bytes(
            oxidged::gedzip::GedzipReader::new(std::io::Cursor::new(bytes))
                .and_then(|mut a| a.read_gedcom_bytes())
                .map_err(|e| CliError::Input(format!("GEDZIP: {e}")))?,
        ),
        Format::Json => return Err(usage("tree reads GEDCOM or GEDZIP")),
    };
    let structures = tree.to_structures();
    Ok(Out {
        text: tree.to_gedcom(),
        json: json!({
            "version": tree.version().to_string(),
            "declared_version": tree.declared_version(),
            "records": serde_json::to_value(&structures).unwrap_or_default(),
        }),
    })
}

/// The encoding a file is decoded with, and the one it declares.
fn encoding(bytes: &[u8]) -> Out {
    let decoded = oxidged::encoding::decode(bytes);
    let bom = bytes.starts_with(b"\xEF\xBB\xBF")
        || bytes.starts_with(b"\xFF\xFE")
        || bytes.starts_with(b"\xFE\xFF");
    let declared = decoded.declared.clone();
    let agrees = declared
        .as_deref()
        .and_then(oxidged::encoding::GedcomEncoding::from_label)
        .map(|d| d == decoded.encoding);
    let mut text = format!("encoding: {}\n", decoded.encoding);
    let _ = writeln!(
        text,
        "declared: {}",
        declared.as_deref().unwrap_or("(nothing)")
    );
    let _ = writeln!(text, "byte order mark: {}", if bom { "yes" } else { "no" });
    if agrees == Some(false) {
        text.push_str("the bytes are not in the declared encoding: evidence wins\n");
    }
    Out {
        text,
        json: json!({
            "encoding": decoded.encoding.to_string(),
            "declared": declared,
            "bom": bom,
            "agrees": agrees,
        }),
    }
}

/// The manual pages.
fn man(out_dir: Option<&std::path::Path>) -> Result<(), CliError> {
    let command = Cli::command();
    let Some(dir) = out_dir else {
        let mut out = Vec::new();
        clap_mangen::Man::new(command).render(&mut out)?;
        return io::write_output(None, &out);
    };
    std::fs::create_dir_all(dir)?;
    clap_mangen::generate_to(command, dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    /// Every command's arguments are well formed (unique names, short
    /// options, defaults among the values).
    #[test]
    fn the_command_line_is_well_formed() {
        super::Cli::command().debug_assert();
    }
}
