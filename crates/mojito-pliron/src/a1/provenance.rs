//! Locations: every operation and block carries an explicit one.
//!
//! The Pliron location names the operation's stable identity, so a parse
//! restores it rather than assigning a position in the assembly buffer.
//! The module and each function are named by their full identity key, and
//! every other operation by its key local to the function it sits in.
//! Canonical text carries the identity in the location alone; the
//! `identity` attribute is restored from it after a parse. Source
//! provenance itself is the typed `provenance` attribute, which this
//! module also reads back.

use std::collections::BTreeMap;

use pliron::context::{Context, Ptr};
use pliron::linked_list::ContainsLinkedList;
use pliron::location::{Located, Location};
use pliron::operation::Operation;

use mojito_common::token::SourceSpan;

use super::attrs::{CoreRole, IdentityAttr, ModuleAttr, ProvenanceAttr, Text};
use super::ops::{KEY_IDENTITY, KEY_PROVENANCE, KEY_TABLES};
use super::verify::{attr, walk};
use super::{A1Error, A1ErrorKind};

/// Give every operation and block of `module` its explicit location.
pub fn stamp_locations(ctx: &Context, module: Ptr<Operation>) -> Result<(), A1Error> {
    for op in walk(ctx, module) {
        let identity: IdentityAttr = attr(ctx, op, &KEY_IDENTITY).ok_or_else(|| {
            A1Error::new(
                A1ErrorKind::Verification,
                format!(
                    "`{}` carries no identity to locate it by",
                    Operation::get_opid(op, ctx)
                ),
            )
        })?;
        if identity.function.as_str().contains('|') {
            return Err(A1Error::new(
                A1ErrorKind::Verification,
                format!(
                    "function symbol `{}` cannot name a location",
                    identity.function
                ),
            ));
        }
        let key = match identity.role {
            CoreRole::Module | CoreRole::Function => identity.key(),
            _ => identity.local_key(),
        };
        if key.contains(['"', '\\']) {
            return Err(A1Error::new(
                A1ErrorKind::Verification,
                format!("identity `{key}` cannot name a location"),
            ));
        }
        op.deref_mut(ctx).set_loc(named(key.clone()));
        for (region_index, region) in op.deref(ctx).regions().enumerate() {
            let blocks: Vec<_> = region.deref(ctx).iter(ctx).collect();
            for (block_index, block) in blocks.into_iter().enumerate() {
                block
                    .deref_mut(ctx)
                    .set_loc(named(format!("{key}#{region_index}.{block_index}")));
            }
        }
    }
    Ok(())
}

/// Give every operation of `module` the identity its location names: the
/// inverse of [`stamp_locations`] with the attribute stripped.
pub fn restore_identities(ctx: &mut Context, module: Ptr<Operation>) -> Result<(), A1Error> {
    for (op, key) in located_keys(ctx, module) {
        let key = key.ok_or_else(|| {
            A1Error::new(
                A1ErrorKind::Parse,
                format!(
                    "`{}` has no location naming its identity",
                    Operation::get_opid(op, ctx)
                ),
            )
        })?;
        let identity = IdentityAttr::from_key(&key).ok_or_else(|| {
            A1Error::new(
                A1ErrorKind::Parse,
                format!("location `{key}` names no identity"),
            )
        })?;
        op.deref_mut(ctx)
            .attributes
            .set((*KEY_IDENTITY).clone(), identity);
    }
    Ok(())
}

/// Remove every operation's `identity` attribute, which its location
/// already names, before `module` prints as canonical text.
pub fn strip_identities(ctx: &mut Context, module: Ptr<Operation>) {
    for op in walk(ctx, module) {
        op.deref_mut(ctx).attributes.0.remove(&*KEY_IDENTITY);
    }
}

/// The source record of every operation, keyed by stable identity.
pub fn location_map(
    ctx: &Context,
    module: Ptr<Operation>,
) -> Result<BTreeMap<String, LocationRecord>, A1Error> {
    let tables: ModuleAttr = attr(ctx, module, &KEY_TABLES)
        .ok_or_else(|| A1Error::new(A1ErrorKind::Verification, "the module carries no tables"))?;
    let mut map = BTreeMap::new();
    for (op, location) in located_keys(ctx, module) {
        let missing = |what: &str| {
            A1Error::new(
                A1ErrorKind::Verification,
                format!("`{}` carries no {what}", Operation::get_opid(op, ctx)),
            )
        };
        let identity: IdentityAttr =
            attr(ctx, op, &KEY_IDENTITY).ok_or_else(|| missing("identity"))?;
        let provenance: ProvenanceAttr =
            attr(ctx, op, &KEY_PROVENANCE).ok_or_else(|| missing("provenance"))?;
        let record = LocationRecord {
            provenance: SourceRecord {
                span: provenance
                    .span
                    .map(|span| span.span(&tables.sources))
                    .transpose()?,
                origin: provenance.origin,
                derived_from: provenance.derived_from,
                reason: provenance.reason,
            },
            location,
        };
        if map.insert(identity.key(), record).is_some() {
            return Err(A1Error::new(
                A1ErrorKind::Verification,
                format!("identity `{}` names two operations", identity.key()),
            ));
        }
    }
    Ok(map)
}

/// What one operation records about where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationRecord {
    pub provenance: SourceRecord,
    /// The identity key the operation's explicit Pliron location names,
    /// with a function-local name read in its function, or `None` when it
    /// has another kind of location.
    pub location: Option<String>,
}

/// An operation's provenance with its source named, not indexed, so a
/// change to one record leaves every other record equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRecord {
    pub span: Option<SourceSpan>,
    pub origin: Option<u32>,
    pub derived_from: Option<Text>,
    pub reason: Text,
}

/// Every operation of `module` with the full identity key its location
/// names: the module's and each function's name is full, and any other
/// name is local to the function before it in nesting order.
fn located_keys(ctx: &Context, module: Ptr<Operation>) -> Vec<(Ptr<Operation>, Option<String>)> {
    let mut function = String::new();
    walk(ctx, module)
        .into_iter()
        .map(|op| {
            let key = location_name(&op.deref(ctx).loc()).map(|name| {
                if op != module && name.starts_with('|') {
                    format!("{function}{name}")
                } else {
                    if op != module {
                        function = name.split('|').next().unwrap_or_default().to_string();
                    }
                    name
                }
            });
            (op, key)
        })
        .collect()
}

fn named(name: String) -> Location {
    Location::Named {
        name,
        child_loc: Box::new(Location::Unknown),
    }
}

fn location_name(location: &Location) -> Option<String> {
    match location {
        Location::Named { name, child_loc } if child_loc.is_unknown() => Some(name.clone()),
        _ => None,
    }
}
