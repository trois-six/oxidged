//! `oxidged edit`: the dataset's edits, each with its references kept
//! whole, the result written as `write` writes.

use clap::{Args, Subcommand, ValueEnum};
use oxidged::model::{
    Dataset, Family, Holder, Individual, Link, Multimedia, Node, RecordHandle, RecordKind, Removal,
    Repository, SharedNote, Source, Submission, Submitter,
};

use crate::io::{ReadArgs, WriteArgs};
use crate::{CliError, usage};

/// An edit of a dataset.
#[derive(Subcommand, Debug)]
pub enum EditCommand {
    /// Rename a record's identifier; every pointer to it follows
    Rename {
        #[command(flatten)]
        read: ReadArgs,
        /// The identifier to rename (e.g. @I1@)
        old: String,
        /// Its new identifier (e.g. @ANN@)
        new: String,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Remove records
    Remove {
        #[command(flatten)]
        read: ReadArgs,
        /// The identifiers of the records to remove
        #[arg(required = true, value_name = "XREF")]
        xrefs: Vec<String>,
        /// What to do with the pointers to a record removed
        #[arg(long, value_enum, default_value_t, value_name = "HOW")]
        pointers: PointersArg,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Point every pointer to a record to another record instead (with
    /// --remove, merge the first into the second)
    Redirect {
        #[command(flatten)]
        read: ReadArgs,
        /// The identifier pointed to now
        #[arg(id = "old_target", value_name = "FROM")]
        from: String,
        /// The identifier to point to instead
        #[arg(id = "new_target", value_name = "TO")]
        to: String,
        /// Then remove the record of FROM, left without pointers
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Remove every structure that points to a record, with its
    /// substructures
    Detach {
        #[command(flatten)]
        read: ReadArgs,
        /// The identifier the structures point to
        xref: String,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Add a pointer from a record (or the header, HEAD) to another
    Link(LinkArgs),
    /// Remove the pointers of a kind from a record (or the header, HEAD) to
    /// another
    Unlink(LinkArgs),
}

/// A pointer to add or remove.
#[derive(Args, Debug)]
pub struct LinkArgs {
    #[command(flatten)]
    read: ReadArgs,
    /// The record that holds the pointer, or HEAD for the header
    holder: String,
    /// The kind of pointer; child and spouse link a family and an
    /// individual both ways (CHIL and FAMC, HUSB or WIFE and FAMS)
    #[arg(value_enum)]
    kind: LinkKind,
    /// The record it points to
    target: String,
    #[command(flatten)]
    write: WriteArgs,
}

/// What `remove` does with the pointers to a record removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum PointersArg {
    /// Refuse to remove a record other structures point to
    #[default]
    Refuse,
    /// Remove the structures that point to it, with their substructures
    Detach,
    /// Leave them pointing to no record
    LeaveDangling,
}

/// A kind of pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LinkKind {
    /// A family's child and the child's family (CHIL and FAMC): FAMILY child INDIVIDUAL
    Child,
    /// A family's partner and the partner's family (HUSB or WIFE, and FAMS):
    /// FAMILY spouse INDIVIDUAL
    Spouse,
    /// An individual's family as a child (FAMC) only
    ChildOf,
    /// An individual's family as a spouse (FAMS) only
    SpouseOf,
    /// A family's husband or first partner (HUSB) only
    Husband,
    /// A family's wife or second partner (WIFE) only
    Wife,
    /// A family's child (CHIL) only
    FamilyChild,
    /// An individual's alias (ALIA)
    Alias,
    /// An association (ASSO)
    Association,
    /// A citation (SOUR) of a source
    Citation,
    /// A shared note (NOTE, 7.x SNOTE)
    Note,
    /// A multimedia link (OBJE)
    Multimedia,
    /// A source's repository (REPO)
    Repository,
    /// A submitter (SUBM)
    Submitter,
    /// An individual's submitter interested in their ancestors (ANCI)
    AncestorInterest,
    /// An individual's submitter interested in their descendants (DESI)
    DescendantInterest,
    /// The header's submission (5.5.1 SUBN)
    Submission,
}

impl LinkKind {
    fn link(self) -> Option<Link> {
        Some(match self {
            LinkKind::Child | LinkKind::Spouse => return None,
            LinkKind::ChildOf => Link::ChildOf,
            LinkKind::SpouseOf => Link::SpouseOf,
            LinkKind::Husband => Link::Husband,
            LinkKind::Wife => Link::Wife,
            LinkKind::FamilyChild => Link::Child,
            LinkKind::Alias => Link::Alias,
            LinkKind::Association => Link::Association,
            LinkKind::Citation => Link::Citation,
            LinkKind::Note => Link::Note,
            LinkKind::Multimedia => Link::Multimedia,
            LinkKind::Repository => Link::Repository,
            LinkKind::Submitter => Link::Submitter,
            LinkKind::AncestorInterest => Link::AncestorInterest,
            LinkKind::DescendantInterest => Link::DescendantInterest,
            LinkKind::Submission => Link::Submission,
        })
    }
}

fn refused(e: oxidged::model::EditError) -> CliError {
    CliError::Edit(e.to_string())
}

fn handle(data: &Dataset, xref: &str) -> Result<RecordHandle, CliError> {
    data.record_handle(xref)
        .ok_or_else(|| CliError::Edit(format!("no record {xref}")))
}

/// Removes the record of a handle, whatever its type.
fn remove(data: &mut Dataset, h: RecordHandle, how: Removal) -> Result<(), CliError> {
    macro_rules! remove {
        ($t:ty) => {
            data.remove(
                h.typed::<$t>()
                    .ok_or_else(|| CliError::Edit("stale record".into()))?,
                how,
            )
            .map(drop)
            .map_err(refused)
        };
    }
    match h.kind() {
        RecordKind::Individual => remove!(Individual),
        RecordKind::Family => remove!(Family),
        RecordKind::Source => remove!(Source),
        RecordKind::Repository => remove!(Repository),
        RecordKind::Multimedia => remove!(Multimedia),
        RecordKind::Submitter => remove!(Submitter),
        RecordKind::Submission => remove!(Submission),
        RecordKind::Note => remove!(SharedNote),
        _ => remove!(Node),
    }
}

fn holder(data: &Dataset, xref: &str) -> Result<Holder, CliError> {
    if xref.eq_ignore_ascii_case("HEAD") {
        Ok(Holder::Header)
    } else {
        handle(data, xref).map(Holder::Record)
    }
}

/// Runs an edit and writes the result.
pub fn run(command: EditCommand, json: bool) -> Result<(), CliError> {
    let (read, write) = match &command {
        EditCommand::Rename { read, write, .. }
        | EditCommand::Remove { read, write, .. }
        | EditCommand::Redirect { read, write, .. }
        | EditCommand::Detach { read, write, .. } => (read.clone(), write.clone()),
        EditCommand::Link(a) | EditCommand::Unlink(a) => (a.read.clone(), a.write.clone()),
    };
    let loaded = read.load(false)?;
    let mut data = loaded.data;
    match command {
        EditCommand::Rename { old, new, .. } => {
            data.rename(old.as_str(), &new).map_err(refused)?;
        }
        EditCommand::Remove {
            xrefs, pointers, ..
        } => {
            let how = match pointers {
                PointersArg::Refuse => Removal::Refuse,
                PointersArg::Detach => Removal::Detach,
                PointersArg::LeaveDangling => Removal::LeaveDangling,
            };
            for xref in &xrefs {
                let h = handle(&data, xref)?;
                remove(&mut data, h, how)?;
            }
        }
        EditCommand::Redirect {
            from,
            to,
            remove: then_remove,
            ..
        } => {
            let n = data.redirect(from.as_str(), to.as_str()).map_err(refused)?;
            eprintln!("redirected: {n}");
            if then_remove {
                let h = handle(&data, &from)?;
                remove(&mut data, h, Removal::Refuse)?;
            }
        }
        EditCommand::Detach { xref, .. } => {
            if data.find(xref.as_str()).is_none() && data.references(xref.as_str()) == 0 {
                return Err(CliError::Edit(format!("nothing points to {xref}")));
            }
            let n = data.detach(xref.as_str());
            eprintln!("removed: {n}");
        }
        EditCommand::Link(a) => link(&mut data, &a, true)?,
        EditCommand::Unlink(a) => link(&mut data, &a, false)?,
    }
    write.write(&data, &loaded.media, json)
}

fn link(data: &mut Dataset, a: &LinkArgs, add: bool) -> Result<(), CliError> {
    let typed = |data: &Dataset, x: &str| -> Result<_, CliError> {
        let h = handle(data, x)?;
        Ok(h)
    };
    match a.kind.link() {
        None => {
            let family = typed(data, &a.holder)?
                .typed::<Family>()
                .ok_or_else(|| usage(format!("{} is not a family", a.holder)))?;
            let individual = typed(data, &a.target)?
                .typed::<Individual>()
                .ok_or_else(|| usage(format!("{} is not an individual", a.target)))?;
            match (a.kind, add) {
                (LinkKind::Child, true) => data.link_child(family, individual).map_err(refused)?,
                (LinkKind::Child, false) => {
                    data.unlink_child(family, individual).map_err(refused)?;
                }
                (_, true) => data.link_spouse(family, individual).map_err(refused)?,
                (_, false) => {
                    data.unlink_spouse(family, individual).map_err(refused)?;
                }
            }
        }
        Some(link) => {
            let holder = holder(data, &a.holder)?;
            if add {
                data.link(holder, link, a.target.as_str())
                    .map_err(refused)?;
            } else {
                data.unlink(holder, link, a.target.as_str())
                    .map_err(refused)?;
            }
        }
    }
    Ok(())
}
