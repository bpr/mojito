//! The values drop elaboration tracks, upstream's `ValueSet`: one range of
//! bits per variable, which drop liveness ([`elaborate_drops`]) carries.
//!
//! A variable is one bit, except a root whose owned parts the body stores
//! into. That root is laid out as upstream lays out a value: at each struct
//! or tuple level a stored path enters, every field in declaration order
//! gets its own range (a further level where a stored path continues, one
//! bit otherwise), and the level's whole-object bit comes last. A store into
//! a laid-out part then defines that part alone, so the value it replaces
//! dies at its own last use rather than at the store.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::OwnedStep;
use mojito_types::types::{Ty, TyKind};
use std::ops::Range;

/// Every variable's bits, and the layout of each root split into parts.
pub(super) struct DropValues {
    /// Variable `v`'s bits are `starts[v]..starts[v + 1]`.
    starts: Vec<u32>,
    layouts: HashMap<VarId, Level>,
}

impl DropValues {
    /// The layout of `f`'s variables: a root `splittable` admits is split
    /// along the owned parts its body stores a value that needs destroying
    /// into, when every level on the way has fields the declarations list.
    pub(super) fn new(
        f: &MirFunction,
        declarations: &MirDeclarations,
        splittable: impl Fn(VarId) -> bool,
    ) -> Self {
        let mut stored: BTreeMap<VarId, (Ty, Vec<Vec<OwnedStep>>)> = BTreeMap::new();
        let mut unsplit: BTreeSet<VarId> = BTreeSet::new();
        for_each_instr_deep(&f.blocks, &mut |instr| {
            let MirInstr::Store { place, .. } = instr else {
                return;
            };
            if !splittable(place.root)
                || !place
                    .ty
                    .as_ref()
                    .is_some_and(|ty| !matches!(ty.kind(), TyKind::Ref(_)) && field_needs_drop(ty))
            {
                return;
            }
            let Some(path) = place.owned_path() else {
                return;
            };
            let Some(root_ty) = place
                .root_ty
                .as_ref()
                .or_else(|| f.var_tys.get(&place.root))
            else {
                unsplit.insert(place.root);
                return;
            };
            stored
                .entry(place.root)
                .or_insert_with(|| (root_ty.clone(), Vec::new()))
                .1
                .push(path);
        });
        let mut starts = Vec::with_capacity(f.n_vars + 1);
        let mut layouts = HashMap::new();
        let mut next = 0;
        for var in 0..f.n_vars {
            let var = var as VarId;
            starts.push(next);
            let layout = stored
                .get(&var)
                .filter(|_| !unsplit.contains(&var))
                .and_then(|(ty, paths)| {
                    let paths: Vec<&[OwnedStep]> = paths.iter().map(Vec::as_slice).collect();
                    let root = MirPlace::root(var, Some(ty.clone()));
                    let mut bit = next;
                    Level::new(ty, &root, &paths, &mut bit, declarations).map(|level| (level, bit))
                });
            match layout {
                Some((level, end)) => {
                    next = end;
                    layouts.insert(var, level);
                }
                None => next += 1,
            }
        }
        starts.push(next);
        Self { starts, layouts }
    }

    /// The bits of variable `v`.
    pub(super) fn range(&self, v: VarId) -> Range<u32> {
        let v = v as usize;
        match (self.starts.get(v), self.starts.get(v + 1)) {
            (Some(start), Some(end)) => *start..*end,
            _ => 0..0,
        }
    }

    /// The variable bit `bit` belongs to.
    pub(super) fn owner(&self, bit: u32) -> VarId {
        (self.starts.partition_point(|start| *start <= bit) - 1) as VarId
    }

    /// Whether `v` is laid out in parts.
    pub(super) fn is_split(&self, v: VarId) -> bool {
        self.layouts.contains_key(&v)
    }

    /// Whether any bit of `v` is in `live`.
    pub(super) fn any_live(&self, live: &DenseSet, v: VarId) -> bool {
        self.range(v).any(|bit| live.contains(bit))
    }

    /// Add every bit of `v` to `live`.
    pub(super) fn mark_all(&self, live: &mut DenseSet, v: VarId) {
        live.extend(self.range(v));
    }

    /// Remove every bit of `v` from `live`.
    pub(super) fn clear_all(&self, live: &mut DenseSet, v: VarId) {
        for bit in self.range(v) {
            live.remove(bit);
        }
    }

    /// A use of the part of `root` at `path` (the whole root when `None`),
    /// made live as upstream's `scheduleNeededDtors` marks it: the walk
    /// drills into a level only while the level's whole-object bit is live,
    /// so the last use of any part of a value that dies whole marks it whole
    /// (`computeAccessPathForMaxUnconsumedField`). Returns the range marked.
    pub(super) fn mark_use(
        &self,
        live: &mut DenseSet,
        root: VarId,
        path: Option<&[OwnedStep]>,
    ) -> Range<u32> {
        let marked = match (self.layouts.get(&root), path) {
            (Some(level), Some(path)) => level.used(live, path),
            _ => self.range(root),
        };
        live.extend(marked.clone());
        marked
    }

    /// The bits of the laid-out part of `root` at exactly `path`.
    pub(super) fn part(&self, root: VarId, path: &[OwnedStep]) -> Option<Range<u32>> {
        self.layouts.get(&root)?.part(path)
    }

    /// Whether a store into `place` defines a laid-out part.
    pub(super) fn defines_part(&self, place: &MirPlace) -> bool {
        place
            .owned_path()
            .is_some_and(|path| self.part(place.root, &path).is_some())
    }

    /// The join of two live sets, upstream's `unifyConsumedSets`: their
    /// union, except that a level whose whole-object bit is live on one side
    /// only, with any of its bits live, is live whole, so the side that does
    /// not keep it destroys it whole and the other side destroys the parts
    /// it redefines.
    pub(super) fn unify(&self, state: &mut DenseSet, other: &DenseSet) {
        let mut whole = Vec::new();
        for level in self.layouts.values() {
            level.unified(state, other, &mut whole);
        }
        state.union(other);
        for range in whole {
            state.extend(range);
        }
    }

    /// The deaths of `root` whose bits in `dying` die here with `live`
    /// live after, as `destroyValueIfNeeded` destroys them: the whole value
    /// when its whole-object bit dies (only when `whole` admits it), and
    /// otherwise each dying part under a live level, in declaration order,
    /// parts whose destruction runs no code skipped.
    pub(super) fn deaths(
        &self,
        root: VarId,
        dying: &DenseSet,
        live: &DenseSet,
        whole: bool,
        out: &mut Vec<Death>,
    ) {
        match self.layouts.get(&root) {
            Some(level) => {
                if !level.range().any(|bit| dying.contains(bit)) {
                    return;
                }
                if !live.contains(level.whole_bit()) {
                    if whole {
                        out.push(Death::var(root));
                    }
                    return;
                }
                level.part_deaths(root, dying, live, out);
            }
            None => {
                if whole && self.range(root).any(|bit| dying.contains(bit)) {
                    out.push(Death::var(root));
                }
            }
        }
    }
}

/// One destruction drop elaboration places: a variable whole, or one owned
/// part of it.
#[derive(Clone)]
pub(super) struct Death {
    pub(super) root: VarId,
    /// The part's first bit, which orders a root's parts as their fields
    /// are declared; zero for a whole variable.
    start: u32,
    place: Option<MirPlace>,
}

impl Death {
    pub(super) const fn var(root: VarId) -> Self {
        Self {
            root,
            start: 0,
            place: None,
        }
    }

    /// The drop: a `DropVar`, or a `DropPlace` of the part, its destructor
    /// named later.
    pub(super) fn into_drop(self) -> MirInstr {
        match self.place {
            None => MirInstr::DropVar {
                var: self.root,
                destructor: None,
            },
            Some(place) => MirInstr::DropPlace {
                place,
                destructor: None,
            },
        }
    }

    /// The sort key of drop order: an initializer's receiver first, then
    /// reverse declaration order, a root's parts in field order.
    pub(super) fn order(&self, receiver: Option<VarId>) -> (bool, std::cmp::Reverse<VarId>, u32) {
        (
            Some(self.root) != receiver,
            std::cmp::Reverse(self.root),
            self.start,
        )
    }
}

/// One struct or tuple level of a split root: its fields' ranges in
/// declaration order, then its whole-object bit.
struct Level {
    start: u32,
    fields: Vec<Field>,
    /// The whole-object bit.
    whole: u32,
}

/// One field of a [`Level`]: its step, its typed place, and its bits.
struct Field {
    step: OwnedStep,
    place: MirPlace,
    /// Whether destroying it runs no code, so no drop of it is placed.
    trivial: bool,
    node: Node,
}

enum Node {
    Leaf(u32),
    Level(Level),
}

impl Level {
    /// The level of a value of type `ty` at `place`, laid out along `paths`
    /// from bit `*next` on; `None` when a level's fields are unknown or a
    /// path names no field.
    fn new(
        ty: &Ty,
        place: &MirPlace,
        paths: &[&[OwnedStep]],
        next: &mut u32,
        declarations: &MirDeclarations,
    ) -> Option<Self> {
        let fields = level_fields(ty, declarations)?;
        if paths
            .iter()
            .any(|path| !fields.iter().any(|(step, _, _)| path.first() == Some(step)))
        {
            return None;
        }
        let start = *next;
        let mut laid = Vec::with_capacity(fields.len());
        for (step, projection, field_ty) in fields {
            let mut field_place = place.clone();
            field_place.project(projection, field_ty.clone());
            let rest: Vec<&[OwnedStep]> = paths
                .iter()
                .filter(|path| path.first() == Some(&step) && path.len() > 1)
                .map(|path| &path[1..])
                .collect();
            let node = if rest.is_empty() {
                *next += 1;
                Node::Leaf(*next - 1)
            } else {
                Node::Level(Self::new(
                    &field_ty,
                    &field_place,
                    &rest,
                    next,
                    declarations,
                )?)
            };
            laid.push(Field {
                step,
                trivial: !field_needs_drop(&field_ty),
                place: field_place,
                node,
            });
        }
        *next += 1;
        Some(Self {
            start,
            fields: laid,
            whole: *next - 1,
        })
    }

    const fn whole_bit(&self) -> u32 {
        self.whole
    }

    const fn range(&self) -> Range<u32> {
        self.start..self.whole + 1
    }

    fn field(&self, step: &OwnedStep) -> Option<&Field> {
        self.fields.iter().find(|field| &field.step == step)
    }

    /// The range a use of the part at `path` marks ([`DropValues::mark_use`]).
    fn used(&self, live: &DenseSet, path: &[OwnedStep]) -> Range<u32> {
        let Some((step, rest)) = path.split_first() else {
            return self.range();
        };
        if !live.contains(self.whole) {
            return self.range();
        }
        match self.field(step).map(|field| &field.node) {
            Some(Node::Leaf(bit)) => *bit..*bit + 1,
            Some(Node::Level(level)) => level.used(live, rest),
            None => self.range(),
        }
    }

    /// The bits of the part at exactly `path`.
    fn part(&self, path: &[OwnedStep]) -> Option<Range<u32>> {
        let (step, rest) = path.split_first()?;
        match (&self.field(step)?.node, rest) {
            (Node::Leaf(bit), []) => Some(*bit..*bit + 1),
            (Node::Level(level), []) => Some(level.range()),
            (Node::Level(level), rest) => level.part(rest),
            (Node::Leaf(_), _) => None,
        }
    }

    /// Collect into `whole` this level's range, and its levels', where
    /// [`DropValues::unify`] makes it live whole.
    fn unified(&self, state: &DenseSet, other: &DenseSet, whole: &mut Vec<Range<u32>>) {
        let both = state.contains(self.whole) && other.contains(self.whole);
        if !both
            && self
                .range()
                .any(|bit| state.contains(bit) || other.contains(bit))
        {
            whole.push(self.range());
            return;
        }
        for field in &self.fields {
            if let Node::Level(level) = &field.node {
                level.unified(state, other, whole);
            }
        }
    }

    /// The deaths of the dying parts under this level, whose whole-object
    /// bit stays live.
    fn part_deaths(&self, root: VarId, dying: &DenseSet, live: &DenseSet, out: &mut Vec<Death>) {
        for field in &self.fields {
            let death = || Death {
                root,
                start: match &field.node {
                    Node::Leaf(bit) => *bit,
                    Node::Level(level) => level.start,
                },
                place: Some(field.place.clone()),
            };
            match &field.node {
                Node::Leaf(bit) => {
                    if dying.contains(*bit) && !field.trivial {
                        out.push(death());
                    }
                }
                Node::Level(level) => {
                    if !level.range().any(|bit| dying.contains(bit)) {
                        continue;
                    }
                    if live.contains(level.whole) {
                        level.part_deaths(root, dying, live, out);
                    } else if !field.trivial {
                        out.push(death());
                    }
                }
            }
        }
    }
}

/// The fields of a value of type `ty` in declaration order, each with its
/// step, its projection, and its type: a tuple's elements, the public
/// `Tuple`'s included, or a struct's fields (`MirDeclarations::field_types`).
fn level_fields(ty: &Ty, declarations: &MirDeclarations) -> Option<Vec<(OwnedStep, Proj, Ty)>> {
    let elements = match ty.kind() {
        TyKind::Tuple(items) => Some(items.iter().collect()),
        kind => mojito_types::types::tuple_elements(kind),
    };
    if let Some(elements) = elements {
        return Some(
            elements
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    (
                        OwnedStep::Element(index),
                        Proj::ConstIndex(index),
                        item.clone(),
                    )
                })
                .collect(),
        );
    }
    Some(
        declarations
            .field_types(ty)?
            .iter()
            .map(|(name, field_ty)| {
                (
                    OwnedStep::Field(name.clone()),
                    Proj::Field(name.clone()),
                    field_ty.clone(),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mojito_mir::mir::{MirBlock, MirTerm};
    use mojito_types::types::tuple_type;

    fn droppable() -> Ty {
        Ty::new(TyKind::Struct("D".into(), Vec::new()))
    }

    /// A body storing into `paths` of variable 0, of type `root`, beside an
    /// untouched variable 1.
    fn values(root: &Ty, paths: &[&[usize]]) -> DropValues {
        let instrs = paths
            .iter()
            .map(|path| {
                let mut place = MirPlace::root(0, Some(root.clone()));
                let mut ty = root.clone();
                for index in *path {
                    let element =
                        mojito_types::types::tuple_elements(ty.kind()).unwrap()[*index].clone();
                    place.project(Proj::ConstIndex(*index), element.clone());
                    ty = element;
                }
                MirInstr::Store { place, src: Reg(0) }
            })
            .collect();
        let function = MirFunction {
            blocks: vec![MirBlock {
                instrs,
                term: MirTerm::Return(None),
            }],
            n_vars: 2,
            ..MirFunction::default()
        };
        DropValues::new(&function, &MirDeclarations::default(), |_| true)
    }

    fn element(index: usize) -> OwnedStep {
        OwnedStep::Element(index)
    }

    #[test]
    fn a_stored_element_gets_its_own_bit_before_the_whole_object_bit() {
        let pair = tuple_type(vec![droppable(), droppable()]);
        let values = values(&pair, &[&[0]]);
        assert!(values.is_split(0));
        assert_eq!(values.range(0), 0..3);
        assert_eq!(values.part(0, &[element(0)]), Some(0..1));
        assert_eq!(values.part(0, &[element(1)]), Some(1..2));
        assert_eq!(values.range(1), 3..4);
        assert_eq!(values.owner(2), 0);
        assert_eq!(values.owner(3), 1);
    }

    #[test]
    fn a_nested_store_lays_out_each_level_it_enters() {
        let inner = tuple_type(vec![droppable(), droppable()]);
        let outer = tuple_type(vec![inner, droppable()]);
        let values = values(&outer, &[&[0, 1]]);
        assert_eq!(values.range(0), 0..5);
        assert_eq!(values.part(0, &[element(0)]), Some(0..3));
        assert_eq!(values.part(0, &[element(0), element(1)]), Some(1..2));
        assert_eq!(values.part(0, &[element(1)]), Some(3..4));
    }

    #[test]
    fn a_use_drills_into_a_level_only_while_its_whole_object_bit_is_live() {
        let pair = tuple_type(vec![droppable(), droppable()]);
        let values = values(&pair, &[&[0]]);
        let mut live = DenseSet::default();
        assert_eq!(values.mark_use(&mut live, 0, Some(&[element(1)])), 0..3);
        live.remove(0);
        assert_eq!(values.mark_use(&mut live, 0, Some(&[element(0)])), 0..1);
    }

    #[test]
    fn a_part_dies_alone_under_a_live_whole_object_bit() {
        let pair = tuple_type(vec![droppable(), droppable()]);
        let values = values(&pair, &[&[0]]);
        let dying: DenseSet = [0].into_iter().collect();
        let live: DenseSet = [1, 2].into_iter().collect();
        let mut deaths = Vec::new();
        values.deaths(0, &dying, &live, true, &mut deaths);
        assert!(matches!(
            deaths.as_slice(),
            [Death { root: 0, place: Some(place), .. }] if place.proj.len() == 1
        ));
        let mut deaths = Vec::new();
        values.deaths(
            0,
            &[0, 1, 2].into_iter().collect(),
            &DenseSet::default(),
            true,
            &mut deaths,
        );
        assert!(matches!(
            deaths.as_slice(),
            [Death {
                root: 0,
                place: None,
                ..
            }]
        ));
    }

    #[test]
    fn unify_makes_a_level_live_whole_where_one_side_drops_it() {
        let pair = tuple_type(vec![droppable(), droppable()]);
        let values = values(&pair, &[&[0]]);
        let mut state: DenseSet = [1, 2].into_iter().collect();
        values.unify(&mut state, &DenseSet::default());
        assert!(state == [0, 1, 2].into_iter().collect());
    }

    #[test]
    fn a_root_with_no_stored_part_is_one_bit() {
        let pair = tuple_type(vec![droppable(), droppable()]);
        let values = values(&pair, &[]);
        assert!(!values.is_split(0));
        assert_eq!(values.range(0), 0..1);
    }
}
