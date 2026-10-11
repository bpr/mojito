//! A package's type section: every distinct type its records hold, once.
//!
//! MLIR bytecode holds each type and attribute once per file, in its
//! attribute/type section, and every op refers to a type by its index; its
//! reader parses an entry the first time something refers to it. A package
//! does the same. Inside [`encoding_types`] a [`Ty`] encodes as the index of
//! its entry, which holds the type's variant with each child type an index
//! in turn, so an entry refers only to entries written before it. Inside
//! [`decoding_types`] a type decodes from its index, its entry the first
//! time any record refers to it. Outside both a type encodes inline, as its
//! variant.
//!
//! An entry is keyed by its own encoding, so two types share an entry only
//! when every part of them encodes alike, the parts their equality leaves
//! out included.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::types::{Ty, TyKind};

/// Run `encode` with types encoding by reference, and give back its result
/// with the type section its records refer to.
pub fn encoding_types<R>(encode: impl FnOnce() -> R) -> (R, Vec<u8>) {
    let previous = SCOPE.replace(Some(Scope::Encoding(Writer::default())));
    let result = encode();
    let Some(Scope::Encoding(writer)) = SCOPE.replace(previous) else {
        unreachable!("the type encoding scope was replaced")
    };
    (result, writer.finish())
}

/// Run `decode` with types decoding from `section`, the section of the
/// records it decodes.
pub fn decoding_types<R>(section: &Arc<TypeSection>, decode: impl FnOnce() -> R) -> R {
    let previous = SCOPE.replace(Some(Scope::Decoding(Arc::clone(section))));
    let result = decode();
    SCOPE.set(previous);
    result
}

/// A package's type section as it is read: its entries in place, each
/// decoded the first time a record refers to it.
pub struct TypeSection {
    bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
    /// Where the entries start in `bytes`.
    start: usize,
    /// Where each entry starts and ends, relative to `start`.
    offsets: Vec<u32>,
    decoded: Vec<OnceLock<Ty>>,
}

impl TypeSection {
    /// The section at `range` of `bytes`.
    ///
    /// # Errors
    ///
    /// When the range leaves `bytes`, or its offset table is malformed.
    pub fn new(
        bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
        range: std::ops::Range<usize>,
    ) -> Result<Self, String> {
        let section = (*bytes)
            .as_ref()
            .get(range.clone())
            .ok_or_else(|| "the type section overruns its package".to_string())?;
        let word = |at: usize| {
            section
                .get(at * 4..at * 4 + 4)
                .and_then(|word| word.try_into().ok())
                .map(u32::from_le_bytes)
                .ok_or_else(|| "the type section is truncated".to_string())
        };
        let count = word(0)? as usize;
        let offsets = (1..=count + 1).map(word).collect::<Result<Vec<_>, _>>()?;
        let start = (count + 2) * 4;
        let monotonic = offsets.windows(2).all(|pair| pair[0] <= pair[1]);
        if offsets.first() != Some(&0)
            || !monotonic
            || start + offsets[count] as usize != section.len()
        {
            return Err("the type section's offsets are malformed".to_string());
        }
        Ok(Self {
            bytes,
            start: range.start + start,
            offsets,
            decoded: (0..count).map(|_| OnceLock::new()).collect(),
        })
    }

    /// How many entries the section holds.
    pub const fn len(&self) -> usize {
        self.decoded.len()
    }

    /// Whether the section holds no entry.
    pub const fn is_empty(&self) -> bool {
        self.decoded.is_empty()
    }

    /// How many entries have been decoded so far.
    pub fn decoded(&self) -> usize {
        self.decoded
            .iter()
            .filter(|entry| entry.get().is_some())
            .count()
    }

    /// The type of entry `index`, decoded on first use.
    ///
    /// # Errors
    ///
    /// When the section has no such entry, or it does not decode.
    pub fn get(self: &Arc<Self>, index: u32) -> Result<Ty, String> {
        let index = index as usize;
        let cell = self
            .decoded
            .get(index)
            .ok_or_else(|| format!("the type section has no entry {index}"))?;
        if let Some(ty) = cell.get() {
            return Ok(ty.clone());
        }
        let entry = (*self.bytes).as_ref().get(
            self.start + self.offsets[index] as usize
                ..self.start + self.offsets[index + 1] as usize,
        );
        let kind: TyKind = decoding_types(self, || {
            postcard::from_bytes(entry.unwrap_or_default())
                .map_err(|error| format!("type entry {index} does not decode: {error}"))
        })?;
        Ok(cell.get_or_init(|| Ty::new(kind)).clone())
    }
}

impl std::fmt::Debug for TypeSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeSection")
            .field("entries", &self.len())
            .field("decoded", &self.decoded())
            .finish()
    }
}

/// The index `ty` encodes as inside [`encoding_types`], interning its entry
/// first; `None` outside it.
pub(crate) fn encoded(ty: &Ty) -> Option<Result<u32, String>> {
    let address = SCOPE.with_borrow(|scope| match scope {
        Some(Scope::Encoding(writer)) => {
            Some(writer.handles.get(&ty.address()).map(|(_, index)| *index))
        }
        _ => None,
    })?;
    if let Some(index) = address {
        return Some(Ok(index));
    }
    // The children intern first, while the variant encodes, so an entry
    // refers only to earlier ones.
    let bytes = match postcard::to_stdvec(ty.kind()) {
        Ok(bytes) => bytes,
        Err(error) => return Some(Err(error.to_string())),
    };
    SCOPE.with_borrow_mut(|scope| {
        let Some(Scope::Encoding(writer)) = scope else {
            unreachable!("the type encoding scope ended while a type encoded")
        };
        Some(Ok(writer.intern(ty, bytes)))
    })
}

/// The section types decode from here, inside [`decoding_types`].
pub(crate) fn decoding_section() -> Option<Arc<TypeSection>> {
    SCOPE.with_borrow(|scope| match scope {
        Some(Scope::Decoding(section)) => Some(Arc::clone(section)),
        _ => None,
    })
}

/// The type section being written.
#[derive(Default)]
struct Writer {
    /// Each entry's encoding, in index order.
    entries: Vec<Vec<u8>>,
    /// The index of each entry, by its encoding.
    indices: HashMap<Vec<u8>, u32>,
    /// The index of each type encoded so far, by its node, which the held
    /// handle keeps from being reused for another type.
    handles: HashMap<usize, (Ty, u32)>,
}

impl Writer {
    fn intern(&mut self, ty: &Ty, bytes: Vec<u8>) -> u32 {
        let next = self.entries.len() as u32;
        let index = *self.indices.entry(bytes).or_insert_with_key(|bytes| {
            self.entries.push(bytes.clone());
            next
        });
        self.handles.insert(ty.address(), (ty.clone(), index));
        index
    }

    /// The section: its entry count, then each entry's start and the last
    /// one's end as 4-byte little-endian offsets, then the entries.
    fn finish(self) -> Vec<u8> {
        let mut section = Vec::new();
        let mut offset = 0u32;
        section.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        section.extend_from_slice(&offset.to_le_bytes());
        for entry in &self.entries {
            offset += entry.len() as u32;
            section.extend_from_slice(&offset.to_le_bytes());
        }
        for entry in self.entries {
            section.extend_from_slice(&entry);
        }
        section
    }
}

/// The type encoding in progress on this thread.
enum Scope {
    Encoding(Writer),
    Decoding(Arc<TypeSection>),
}

thread_local! {
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TyArg, TyKind};

    #[test]
    fn a_section_holds_each_type_once_and_decodes_it_shared() {
        let element = Ty::new(TyKind::Struct("W".into(), vec![TyArg::Ty(Ty::int())]));
        let nested = Ty::new(TyKind::Struct("W".into(), vec![TyArg::Ty(element.clone())]));
        let values = vec![nested.clone(), element, nested, Ty::int()];
        let (encoded, section) = encoding_types(|| postcard::to_stdvec(&values));
        let encoded = encoded.expect("encodes");
        // Int, W[Int], W[W[Int]]: one entry each.
        assert_eq!(u32::from_le_bytes(section[..4].try_into().unwrap()), 3);
        let length = section.len();
        let section = Arc::new(
            TypeSection::new(Arc::new(section), 0..length).expect("a well-formed section"),
        );
        let decoded: Vec<Ty> =
            decoding_types(&section, || postcard::from_bytes(&encoded)).expect("decodes");
        assert_eq!(decoded, values);
        assert!(decoded[0].ptr_eq(&decoded[2]));
        let TyKind::Struct(_, arguments) = decoded[0].kind() else {
            panic!("a struct type");
        };
        assert!(matches!(&arguments[0], TyArg::Ty(inner) if inner.ptr_eq(&decoded[1])));
    }

    #[test]
    fn a_malformed_section_is_refused() {
        let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(vec![1u8, 0, 0, 0, 5, 0, 0, 0]);
        assert!(TypeSection::new(Arc::clone(&bytes), 0..8).is_err());
        assert!(TypeSection::new(bytes, 0..9).is_err());
    }
}
