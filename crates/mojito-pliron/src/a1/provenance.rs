//! Locations: every operation and block carries an explicit one.
//!
//! The Pliron location names the operation's stable identity, so a parse
//! restores it rather than assigning a position in the assembly buffer.
//! Source provenance itself is the typed `provenance` attribute, which
//! this module also reads back.

use std::collections::BTreeMap;

use pliron::context::{Context, Ptr};
use pliron::linked_list::ContainsLinkedList;
use pliron::location::{Located, Location};
use pliron::operation::Operation;

use super::attrs::{IdentityAttr, ProvenanceAttr};
use super::ops::{KEY_IDENTITY, KEY_PROVENANCE};
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
        let key = identity.key();
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

/// The source record of every operation, keyed by stable identity.
pub fn location_map(
    ctx: &Context,
    module: Ptr<Operation>,
) -> Result<BTreeMap<String, LocationRecord>, A1Error> {
    let mut map = BTreeMap::new();
    for op in walk(ctx, module) {
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
            provenance,
            location: location_name(&op.deref(ctx).loc()),
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
    pub provenance: ProvenanceAttr,
    /// The name of the operation's explicit Pliron location, or `None`
    /// when it has another kind of location.
    pub location: Option<String>,
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
