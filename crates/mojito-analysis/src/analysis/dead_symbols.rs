//! `eliminate-dead-symbols`: upstream's `EliminateDeadSymbols`, the
//! symbol DCE its pre-elaboration pipeline runs after destructor insertion,
//! over a program's own MIR.
//!
//! Upstream roots the walk at the `export` symbols and removes every other
//! symbol no live symbol refers to. Here the roots are the program's entry
//! roots and the symbols its compilation exports by name (a native entry
//! a caller compiles, such as a scalar `compute`): every function and
//! struct nothing reachable from them names goes, its declaration with it.

use super::pre_elaboration::ProgramPass;
use mojito_common::timing;
use mojito_mir::mir::{
    LIFECYCLE_MEMBERS, MirFunction, MirFunctionDeclaration, MirProgram, MirStructDeclaration,
    SymbolTable, destroyed_structs, entry_roots, relocated_structs,
};
use mojito_symbol::symbol::{OV_SEP, split_method_symbol};
use mojito_types::types::TyKind;
use std::collections::{HashMap, HashSet};

/// The pass, under upstream's flag spelling.
pub const ELIMINATE_DEAD_SYMBOLS: ProgramPass = ProgramPass {
    name: "eliminate-dead-symbols",
    run: eliminate_dead_symbols,
};

/// Remove `program`'s functions and structs that neither its entry roots
/// nor `exports` reach, with their declarations, keeping the rest in order.
///
/// A live function makes live every symbol its body or declaration names,
/// every member of its overload family, every function nested in it, the
/// destructor of every struct its drops may tear down
/// ([`destroyed_structs`]), and the copy and move constructors of every
/// struct it may copy or move ([`relocated_structs`]), as upstream's
/// destructor and copy/move insertion leaves a call to each at its site. A
/// live struct makes live every symbol its declaration names and its other
/// methods: the elaborator reaches a struct's constructors, display, hash,
/// and trait-requirement methods from its type alone, as upstream nests a
/// struct's members under its symbol. A body that drops, copies, or moves a
/// value of a type only an instantiation resolves, or a program that
/// imports bodies the walk cannot see, keeps every live struct's
/// destructor, copy, and move constructors.
pub fn eliminate_dead_symbols(program: &mut MirProgram, exports: &[String]) {
    let (live, live_structs) = {
        let (functions, structs) = Symbols::of(program).live(exports);
        let owned = |names: HashSet<&str>| -> HashSet<String> {
            names.into_iter().map(str::to_string).collect()
        };
        (owned(functions), owned(structs))
    };
    let mut dead = HashSet::new();
    let mut instructions = 0;
    program.functions.retain(|(name, function)| {
        let keep = live.contains(name);
        if !keep {
            instructions += function.instruction_count();
            dead.insert(name.clone());
        }
        keep
    });
    let declarations = &mut program.declarations;
    declarations
        .functions
        .retain(|declaration| !dead.contains(&declaration.lowered_name));
    let structs = declarations.structs.len();
    declarations
        .structs
        .retain(|declaration| live_structs.contains(&declaration.name));
    for declaration in &mut declarations.structs {
        declaration
            .members
            .retain(|entry| !dead.contains(&entry.symbol));
    }
    timing::count("dead_functions", dead.len() as u64);
    timing::count("dead_instructions", instructions as u64);
    timing::count(
        "dead_structs",
        (structs - declarations.structs.len()) as u64,
    );
}

/// A program's own symbols, indexed for the liveness walk.
struct Symbols<'p> {
    program: &'p MirProgram,
    functions: HashMap<&'p str, &'p MirFunction>,
    declarations: HashMap<&'p str, &'p MirFunctionDeclaration>,
    structs: HashMap<&'p str, &'p MirStructDeclaration>,
    /// Every own function and struct name: what a use may name.
    table: SymbolTable<'p>,
    /// Each own struct's methods but its lifecycle members, and the
    /// dispatched members its table names.
    methods: HashMap<&'p str, Vec<&'p str>>,
    /// Each own struct's destructor, copy, and move constructors, by the
    /// struct and the member's name, as its member table names them.
    lifecycle: HashMap<(&'p str, &'p str), &'p str>,
    /// Each overload family's members, by the family's base name.
    families: HashMap<&'p str, Vec<&'p str>>,
    /// The functions nested in each function.
    nested: HashMap<&'p str, Vec<&'p str>>,
}

impl<'p> Symbols<'p> {
    fn of(program: &'p MirProgram) -> Self {
        let functions: HashMap<_, _> = program
            .functions
            .iter()
            .map(|(name, function)| (name.as_str(), function))
            .collect();
        let declarations: HashMap<_, _> = program
            .declarations
            .functions
            .iter()
            .map(|declaration| (declaration.lowered_name.as_str(), declaration))
            .collect();
        let structs: HashMap<_, _> = program
            .declarations
            .structs
            .iter()
            .map(|declaration| (declaration.name.as_str(), declaration))
            .collect();
        let table = SymbolTable::new(functions.keys().chain(structs.keys()).copied());
        let mut lifecycle = HashMap::new();
        for declaration in &program.declarations.structs {
            for entry in &declaration.members {
                if let Some(member) = LIFECYCLE_MEMBERS
                    .into_iter()
                    .find(|member| *member == entry.member)
                    && let Some((&symbol, _)) = functions.get_key_value(entry.symbol.as_str())
                {
                    lifecycle.insert((declaration.name.as_str(), member), symbol);
                }
            }
        }
        let members: HashSet<&str> = lifecycle.values().copied().collect();
        let mut methods: HashMap<_, Vec<_>> = HashMap::new();
        // A dispatched member a struct's table names is its method whatever
        // its spelling: an instance that removed the struct's parameters
        // names no struct instance.
        for declaration in &program.declarations.structs {
            for entry in &declaration.members {
                if !LIFECYCLE_MEMBERS.contains(&entry.member.as_str())
                    && let Some((&symbol, _)) = functions.get_key_value(entry.symbol.as_str())
                {
                    methods
                        .entry(declaration.name.as_str())
                        .or_default()
                        .push(symbol);
                }
            }
        }
        let mut families: HashMap<_, Vec<_>> = HashMap::new();
        let mut nested: HashMap<_, Vec<_>> = HashMap::new();
        for (name, _) in &program.functions {
            let name = name.as_str();
            if let Some((owner, _)) = split_method_symbol(name)
                && structs.contains_key(owner)
                && !members.contains(name)
            {
                methods.entry(owner).or_default().push(name);
            }
            families.entry(family(name)).or_default().push(name);
            if let Some(enclosing) = declarations
                .get(name)
                .and_then(|declaration| declaration.enclosing.as_deref())
            {
                nested.entry(enclosing).or_default().push(name);
            }
        }
        Self {
            program,
            functions,
            declarations,
            structs,
            table,
            methods,
            lifecycle,
            families,
            nested,
        }
    }

    /// The live functions and structs: the roots, the entry roots and
    /// `exports`, and everything they reach.
    fn live(mut self, exports: &[String]) -> (HashSet<&'p str>, HashSet<&'p str>) {
        let mut pending: Vec<&'p str> = entry_roots(self.program)
            .iter()
            .chain(exports)
            .filter_map(|root| self.functions.get_key_value(root.as_str()))
            .map(|(&name, _)| name)
            .collect();
        let mut live = HashSet::new();
        let mut live_structs: HashSet<&'p str> = HashSet::new();
        // Set once a live body drops, copies, or moves a value of a
        // symbolic type. A body the program imports is not walked, and may
        // do so to any value.
        let mut every_member = self.program.declarations.imported.is_some();
        while let Some(name) = pending.pop() {
            if let Some(function) = self.functions.get(name)
                && live.insert(name)
            {
                pending.extend(self.table.function_uses(function));
                let fields = |name: &str| {
                    self.structs
                        .get(name)
                        .map(|declaration| declaration.fields.as_slice())
                };
                let destroyed = destroyed_structs(function, &fields);
                let relocated = relocated_structs(function, &fields);
                if (destroyed.symbolic || relocated.symbolic) && !every_member {
                    every_member = true;
                    for &owner in &live_structs {
                        pending.extend(self.members(owner));
                    }
                }
                for (member, structs) in [
                    ("__deinit__", &destroyed.structs),
                    ("__copyinit__", &relocated.copied),
                    ("__moveinit__", &relocated.moved),
                ] {
                    pending.extend(structs.iter().filter_map(|ty| match ty.kind() {
                        TyKind::Struct(name, _) => {
                            self.lifecycle.get(&(name.as_str(), member)).copied()
                        }
                        _ => None,
                    }));
                }
                if let Some(declaration) = self.declarations.get(name) {
                    pending.extend(self.table.uses(*declaration));
                }
                pending.extend(self.families.get(family(name)).into_iter().flatten());
                pending.extend(self.nested.get(name).into_iter().flatten());
            }
            if let Some(declaration) = self.structs.get(name)
                && live_structs.insert(name)
            {
                pending.extend(self.table.uses(*declaration));
                pending.extend(self.methods.get(name).into_iter().flatten());
                if every_member {
                    pending.extend(self.members(name));
                }
            }
        }
        (live, live_structs)
    }

    /// The struct `owner`'s destructor, copy, and move constructors.
    fn members(&self, owner: &'p str) -> impl Iterator<Item = &'p str> + '_ {
        LIFECYCLE_MEMBERS
            .iter()
            .filter_map(move |&member| self.lifecycle.get(&(owner, member)).copied())
    }
}

/// The overload family `name` belongs to: its spelling before the overload
/// qualifier.
fn family(name: &str) -> &str {
    name.split_once(OV_SEP).map_or(name, |(base, _)| base)
}
