//! The values drop elaboration tracks, upstream's `ValueSet`: one range of
//! bits per variable, which drop liveness ([`elaborate_drops`]) carries.
//!
//! A variable is one bit, except a root whose owned parts the body stores
//! into, and a `deinit` parameter. That root is laid out as upstream lays
//! out a value: at each struct or tuple level a stored path enters, every
//! field in declaration order gets its own range (a further level where a
//! stored path continues, one bit otherwise), and the level's whole-object
//! bit comes last; a `deinit` parameter's every field enters its first
//! level. A store into a laid-out part then defines that part alone, so the
//! value it replaces dies at its own last use rather than at the store, and
//! a `deinit` parameter's fields die one by one, each at its own last use.
//!
//! Every owned register ([`mojito_mir::mir::RegisterEnds`]) is one bit
//! after the variables', as upstream's `ValueSet::addValue` numbers an owned
//! SSA value: register `k`-th in register order is root `n_vars + k`.
//!
//! One bit past every value's is upstream's sentinel slot: set where a
//! path reaches an exit, so a live set without it is unreached, which a
//! join takes the other side over.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::OwnedStep;
use mojito_types::types::{Ty, TyKind};
use std::ops::Range;

/// Every variable's bits, each owned register's, and the layout of each
/// root split into parts.
pub(super) struct DropValues {
    /// Root `v`'s bits are `starts[v]..starts[v + 1]`; the last start is
    /// the sentinel bit, set where a path reaches an exit.
    starts: Vec<u32>,
    layouts: HashMap<VarId, Level>,
    /// The number of variables: roots from here on are registers.
    vars: VarId,
    /// The owned registers, in root order.
    registers: Vec<Reg>,
    /// Each owned register's root.
    register_roots: HashMap<u32, VarId>,
    /// For each register root, one past the highest variable defined or
    /// declared before its definition, upstream's discovery order.
    anchors: Vec<u32>,
}

impl DropValues {
    /// The layout of `f`'s variables and its `owned` registers: a root
    /// `splittable` admits is split along the owned parts its body stores a
    /// value that needs destroying into, and, when `fielded` admits it too,
    /// along each of its fields, when every level on the way has fields the
    /// declarations list; a register is one bit.
    pub(super) fn new(
        f: &MirFunction,
        declarations: &MirDeclarations,
        owned: &[Reg],
        splittable: impl Fn(VarId) -> bool,
        fielded: impl Fn(VarId) -> bool,
    ) -> Self {
        let mut stored: BTreeMap<VarId, (Ty, Vec<Vec<OwnedStep>>)> = BTreeMap::new();
        let mut unsplit: BTreeSet<VarId> = BTreeSet::new();
        for var in (0..f.n_vars as VarId).filter(|v| splittable(*v) && fielded(*v)) {
            let Some(ty) = f.var_tys.get(&var) else {
                continue;
            };
            if let Some(fields) = level_fields(ty, declarations) {
                let paths = fields.into_iter().map(|(step, _, _)| vec![step]).collect();
                stored.insert(var, (ty.clone(), paths));
            }
        }
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
        let vars = f.n_vars as VarId;
        let mut register_roots = HashMap::new();
        for (index, reg) in owned.iter().enumerate() {
            starts.push(next);
            next += 1;
            register_roots.insert(reg.0, vars + index as VarId);
        }
        starts.push(next);
        let anchors = register_anchors(f, owned);
        Self {
            starts,
            layouts,
            vars,
            registers: owned.to_vec(),
            register_roots,
            anchors,
        }
    }

    /// The root of `reg`, when it is an owned register.
    pub(super) fn register_root(&self, reg: Reg) -> Option<VarId> {
        self.register_roots.get(&reg.0).copied()
    }

    /// Whether `root` is an owned register's.
    pub(super) const fn is_register_root(&self, root: VarId) -> bool {
        root >= self.vars
    }

    /// The first register root: roots below it are variables'.
    pub(super) const fn first_register(&self) -> VarId {
        self.vars
    }

    /// The register whose root `root` is.
    fn root_register(&self, root: VarId) -> Option<Reg> {
        root.checked_sub(self.vars)
            .and_then(|index| self.registers.get(index as usize).copied())
    }

    /// The whole of each owned register of `regs`.
    pub(super) fn register_refs(&self, regs: impl IntoIterator<Item = Reg>) -> Vec<ValueRef> {
        regs.into_iter()
            .filter_map(|reg| self.register_root(reg))
            .map(|root| self.whole(root))
            .collect()
    }

    /// Where `root` dies among the deaths at one point: a variable `v` at
    /// `(v + 1, 0)`, and a register after the variables declared after its
    /// definition and before those declared before it, as upstream destroys
    /// in descending discovery order.
    fn rank(&self, root: VarId) -> (u32, u32) {
        match root.checked_sub(self.vars) {
            Some(index) => (
                self.anchors
                    .get(index as usize)
                    .copied()
                    .unwrap_or_default(),
                index + 1,
            ),
            None => (root + 1, 0),
        }
    }

    /// The sentinel bit: set in a live set a path to an exit reaches.
    fn reached(&self) -> u32 {
        self.starts.last().copied().unwrap_or_default()
    }

    /// Mark `live` as reached by a path to an exit, as upstream's
    /// `checkTerminatorOp` sets slot 0 at a `return` or a raise.
    pub(super) fn mark_reached(&self, live: &mut DenseSet) {
        live.insert(self.reached());
    }

    /// Whether `bit` is a variable's, not the sentinel.
    pub(super) fn is_value_bit(&self, bit: u32) -> bool {
        bit < self.reached()
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

    /// The whole of variable `v`.
    pub(super) fn whole(&self, v: VarId) -> ValueRef {
        let bits = self.range(v);
        ValueRef {
            root: v,
            start: bits.start,
            end: bits.end,
        }
    }

    /// The part `place` reaches, upstream's `getValueRefsForAccess`: the
    /// deepest laid-out part its leading field and element steps name, the
    /// whole root when it is not laid out or the first step names no field.
    /// A `through` reference is the caller's to add.
    pub(super) fn access(&self, place: &MirPlace) -> ValueRef {
        let Some(level) = self.layouts.get(&place.root) else {
            return self.whole(place.root);
        };
        let steps = place.proj.iter().map_while(|step| match step {
            Proj::Field(name) => Some(OwnedStep::Field(name.clone())),
            Proj::ConstIndex(index) => Some(OwnedStep::Element(*index)),
            _ => None,
        });
        let bits = level.reached(steps);
        ValueRef {
            root: place.root,
            start: bits.start,
            end: bits.end,
        }
    }

    /// The bits a use of `part` makes live in `live`, as upstream's
    /// `scheduleNeededDtors` marks them: the walk drills into a level only
    /// while the level's whole-object bit is live, so the last use of any
    /// part of a value that dies whole marks it whole
    /// (`computeAccessPathForMaxUnconsumedField`).
    pub(super) fn ref_bits(&self, live: &DenseSet, part: ValueRef) -> Range<u32> {
        match self.layouts.get(&part.root) {
            Some(level) => level.used(live, part.bits()),
            None => self.range(part.root),
        }
    }

    /// Make a use of `part` live ([`Self::ref_bits`]); returns the range
    /// marked.
    pub(super) fn mark_ref(&self, live: &mut DenseSet, part: ValueRef) -> Range<u32> {
        let marked = self.ref_bits(live, part);
        live.extend(marked.clone());
        marked
    }

    /// Make the whole-object bit of `v` live alone, upstream's
    /// `memMarkDestroyed`: a later use of a part then drills to that part.
    /// An unsplit `v` is its one bit.
    pub(super) fn mark_whole_bit(&self, live: &mut DenseSet, v: VarId) {
        match self.layouts.get(&v) {
            Some(level) => {
                live.insert(level.whole_bit());
            }
            None => self.mark_all(live, v),
        }
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

    /// The join of two live sets, upstream's `unifyConsumedSets`: the
    /// reached side when the other is unreached, else their union, except
    /// that a level whose whole-object bit is live on one side only, with
    /// any of its bits live, is live whole, so the side that does not keep
    /// it destroys it whole and the other side destroys the parts it
    /// redefines.
    pub(super) fn unify(&self, state: &mut DenseSet, other: &DenseSet) {
        if !other.contains(self.reached()) {
            return;
        }
        if !state.contains(self.reached()) {
            state.clone_from(other);
            return;
        }
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
                        out.push(Death::var(root, self.range(root)));
                    }
                    return;
                }
                level.part_deaths(root, dying, live, out);
            }
            None => {
                if whole && self.range(root).any(|bit| dying.contains(bit)) {
                    out.push(Death {
                        register: self.root_register(root),
                        rank: self.rank(root),
                        ..Death::var(root, self.range(root))
                    });
                }
            }
        }
    }
}

/// For each of `owned`, one past the highest variable defined or declared
/// before its definition, in a pre-order walk of `f`'s body.
fn register_anchors(f: &MirFunction, owned: &[Reg]) -> Vec<u32> {
    let index: HashMap<u32, usize> = owned
        .iter()
        .enumerate()
        .map(|(index, reg)| (reg.0, index))
        .collect();
    let mut anchors = vec![f.n_params as u32; owned.len()];
    let mut seen = f.n_params as u32;
    let mut results = Vec::new();
    for_each_instr_deep(&f.blocks, &mut |instr| {
        if let Some(var) = var_def(instr).or(match instr {
            MirInstr::DeclareVar { var } => Some(*var),
            _ => None,
        }) {
            seen = seen.max(var + 1);
        }
        results.clear();
        mojito_mir::mir::verify::instruction_result_regs(instr, &mut results);
        results.extend(mojito_mir::mir::region_argument_regs(instr));
        for reg in &results {
            if let Some(&at) = index.get(&reg.0) {
                anchors[at] = seen;
            }
        }
    });
    anchors
}

/// A reference to bits of one variable, upstream's `ValueRef`: the whole
/// variable ([`DropValues::whole`]) or one laid-out part of it
/// ([`DropValues::access`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ValueRef {
    pub(super) root: VarId,
    start: u32,
    end: u32,
}

impl ValueRef {
    /// Its bits.
    pub(super) const fn bits(self) -> Range<u32> {
        self.start..self.end
    }

    /// Whether any of its bits is in `bits`.
    pub(super) const fn overlaps(self, bits: &Range<u32>) -> bool {
        overlap(&self.bits(), bits)
    }
}

/// Whether two ranges of bits share one.
pub(super) const fn overlap(a: &Range<u32>, b: &Range<u32>) -> bool {
    a.start < b.end && b.start < a.end
}

/// One destruction drop elaboration places: a variable whole, one owned
/// part of it, or an owned register.
#[derive(Clone)]
pub(super) struct Death {
    pub(super) root: VarId,
    /// Its bits: the variable's, or the part's, whose first bit orders a
    /// root's parts as their fields are declared.
    bits: Range<u32>,
    place: Option<MirPlace>,
    /// The register, when the root is an owned register's.
    register: Option<Reg>,
    /// Its place among the deaths at one point ([`DropValues::rank`]).
    rank: (u32, u32),
}

impl Death {
    /// The death of variable `root`, whose bits are `bits`, whole.
    pub(super) const fn var(root: VarId, bits: Range<u32>) -> Self {
        Self {
            root,
            bits,
            place: None,
            register: None,
            rank: (root + 1, 0),
        }
    }

    /// Its bits.
    pub(super) fn bits(&self) -> Range<u32> {
        self.bits.clone()
    }

    /// The drop: a `DropVar`, a `DropPlace` of the part, or a `Drop` of the
    /// register, its destructor named later.
    pub(super) fn into_drop(self) -> MirInstr {
        match (self.register, self.place) {
            (Some(reg), _) => MirInstr::Drop {
                reg,
                destructor: None,
            },
            (None, None) => MirInstr::DropVar {
                var: self.root,
                destructor: None,
            },
            (None, Some(place)) => MirInstr::DropPlace {
                place,
                destructor: None,
            },
        }
    }

    /// The sort key of drop order: an initializer's receiver first, then
    /// reverse discovery order ([`DropValues::rank`]), a root's parts in
    /// field order.
    pub(super) fn order(
        &self,
        receiver: Option<VarId>,
    ) -> (bool, std::cmp::Reverse<(u32, u32)>, u32) {
        (
            Some(self.root) != receiver,
            std::cmp::Reverse(self.rank),
            self.bits.start,
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

    /// The range a use of the part whose bits are `part` marks
    /// ([`DropValues::ref_bits`]).
    fn used(&self, live: &DenseSet, part: Range<u32>) -> Range<u32> {
        if part == self.range() || !live.contains(self.whole) {
            return self.range();
        }
        let field = self.fields.iter().find(|field| match &field.node {
            Node::Leaf(bit) => part.start == *bit,
            Node::Level(level) => level.range().contains(&part.start),
        });
        match field.map(|field| &field.node) {
            Some(Node::Leaf(bit)) => *bit..*bit + 1,
            Some(Node::Level(level)) => level.used(live, part),
            None => self.range(),
        }
    }

    /// The bits of the deepest part `steps` reach: a leaf ends the walk, and
    /// a step that names no field stops it at the level it would enter.
    fn reached(&self, mut steps: impl Iterator<Item = OwnedStep>) -> Range<u32> {
        let Some(step) = steps.next() else {
            return self.range();
        };
        match self.field(&step).map(|field| &field.node) {
            Some(Node::Leaf(bit)) => *bit..*bit + 1,
            Some(Node::Level(level)) => level.reached(steps),
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
                bits: match &field.node {
                    Node::Leaf(bit) => *bit..*bit + 1,
                    Node::Level(level) => level.range(),
                },
                place: Some(field.place.clone()),
                ..Death::var(root, 0..0)
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
/// step, its projection, and its type: a tuple's elements, or a struct's
/// fields (`MirDeclarations::field_types`, or the declared ones of a
/// struct's own `Self`), the public `Tuple`'s `storage` included.
fn level_fields(ty: &Ty, declarations: &MirDeclarations) -> Option<Vec<(OwnedStep, Proj, Ty)>> {
    if let TyKind::Tuple(items) = ty.kind() {
        return Some(
            items
                .iter()
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
    // A struct named without arguments in its own body is its `Self`, whose
    // fields are the declared ones, over its own parameters.
    let declared = || match ty.kind() {
        TyKind::Struct(name, arguments) if arguments.is_empty() => declarations
            .structure(name)
            .map(|structure| std::borrow::Cow::Borrowed(structure.fields.as_slice())),
        _ => None,
    };
    Some(
        declarations
            .field_types(ty)
            .or_else(declared)?
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

    fn tuple(items: Vec<Ty>) -> Ty {
        Ty::new(TyKind::Tuple(items))
    }

    fn droppable() -> Ty {
        Ty::new(TyKind::Struct("D".into(), Vec::new()))
    }

    /// The place of the elements at `path` of variable 0, of type `root`.
    fn place(root: &Ty, path: &[usize]) -> MirPlace {
        let mut place = MirPlace::root(0, Some(root.clone()));
        let mut ty = root.clone();
        for index in path {
            let TyKind::Tuple(items) = ty.kind() else {
                break;
            };
            let element = items[*index].clone();
            place.project(Proj::ConstIndex(*index), element.clone());
            ty = element;
        }
        place
    }

    /// A body storing into `paths` of variable 0, of type `root`, beside an
    /// untouched variable 1.
    fn values(root: &Ty, paths: &[&[usize]]) -> DropValues {
        let instrs = paths
            .iter()
            .map(|path| MirInstr::Store {
                place: place(root, path),
                src: Reg(0),
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
        DropValues::new(
            &function,
            &MirDeclarations::default(),
            &[],
            |_| true,
            |_| false,
        )
    }

    fn element(index: usize) -> OwnedStep {
        OwnedStep::Element(index)
    }

    #[test]
    fn a_stored_element_gets_its_own_bit_before_the_whole_object_bit() {
        let pair = tuple(vec![droppable(), droppable()]);
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
        let inner = tuple(vec![droppable(), droppable()]);
        let outer = tuple(vec![inner, droppable()]);
        let values = values(&outer, &[&[0, 1]]);
        assert_eq!(values.range(0), 0..5);
        assert_eq!(values.part(0, &[element(0)]), Some(0..3));
        assert_eq!(values.part(0, &[element(0), element(1)]), Some(1..2));
        assert_eq!(values.part(0, &[element(1)]), Some(3..4));
    }

    #[test]
    fn a_use_drills_into_a_level_only_while_its_whole_object_bit_is_live() {
        let pair = tuple(vec![droppable(), droppable()]);
        let values = values(&pair, &[&[0]]);
        let mut live = DenseSet::default();
        let second = values.access(&place(&pair, &[1]));
        assert_eq!(values.mark_ref(&mut live, second), 0..3);
        live.remove(0);
        let first = values.access(&place(&pair, &[0]));
        assert_eq!(values.mark_ref(&mut live, first), 0..1);
    }

    #[test]
    fn an_access_reaches_the_deepest_laid_out_part() {
        let inner = tuple(vec![droppable(), droppable()]);
        let outer = tuple(vec![inner, droppable()]);
        let values = values(&outer, &[&[0, 1]]);
        assert_eq!(values.access(&place(&outer, &[0, 1])).bits(), 1..2);
        assert_eq!(values.access(&place(&outer, &[0])).bits(), 0..3);
        assert_eq!(values.access(&place(&outer, &[1])).bits(), 3..4);
        assert_eq!(values.access(&place(&outer, &[])), values.whole(0));
    }

    #[test]
    fn an_access_past_a_leaf_stops_at_the_leaf() {
        let inner = tuple(vec![droppable(), droppable()]);
        let outer = tuple(vec![inner, droppable()]);
        let values = values(&outer, &[&[1]]);
        assert_eq!(values.access(&place(&outer, &[0, 1])).bits(), 0..1);
    }

    #[test]
    fn a_part_under_a_dead_whole_object_bit_marks_its_level() {
        let inner = tuple(vec![droppable(), droppable()]);
        let outer = tuple(vec![inner, droppable()]);
        let values = values(&outer, &[&[0, 1]]);
        let part = values.access(&place(&outer, &[0, 1]));
        let mut live: DenseSet = std::iter::once(4).collect();
        assert_eq!(values.mark_ref(&mut live, part), 0..3);
        let mut live: DenseSet = [2, 4].into_iter().collect();
        assert_eq!(values.mark_ref(&mut live, part), 1..2);
    }

    #[test]
    fn a_part_dies_alone_under_a_live_whole_object_bit() {
        let pair = tuple(vec![droppable(), droppable()]);
        let values = values(&pair, &[&[0]]);
        let dying: DenseSet = std::iter::once(0).collect();
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
        let pair = tuple(vec![droppable(), droppable()]);
        let values = values(&pair, &[&[0]]);
        let mut state: DenseSet = [1, 2, 4].into_iter().collect();
        values.unify(&mut state, &std::iter::once(4).collect());
        assert!(state == [0, 1, 2, 4].into_iter().collect());
    }

    #[test]
    fn unify_takes_the_reached_side_over_an_unreached_one() {
        let pair = tuple(vec![droppable(), droppable()]);
        let values = values(&pair, &[&[0]]);
        let reached: DenseSet = [1, 2, 4].into_iter().collect();
        let mut state = reached.clone();
        values.unify(&mut state, &DenseSet::default());
        assert!(state == reached);
        let mut state = DenseSet::default();
        values.unify(&mut state, &reached);
        assert!(state == reached);
    }

    #[test]
    fn a_root_with_no_stored_part_is_one_bit() {
        let pair = tuple(vec![droppable(), droppable()]);
        let values = values(&pair, &[]);
        assert!(!values.is_split(0));
        assert_eq!(values.range(0), 0..1);
    }
}
