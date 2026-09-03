//! Mechanical ELF inspection (Stage 6, S6.4.5): pure-Rust (`object` crate)
//! reads of emitted artifacts for tests and diagnostics — per-section
//! digests for reproducibility diffs, symbol surfaces, and the
//! machine/PIE/executable-stack facts the artifact contract pins. Shared by
//! the repro/dist test lanes and error reporting; never load-bearing for
//! emission itself.

use std::path::Path;

use object::{Object, ObjectSection, ObjectSymbol};

/// One row per allocated/code/data section: `(name, size, sha256)`. The
/// reproducibility tests print a diff of two reports when whole-file bytes
/// mismatch, so failures name the drifting section instead of a bare hash.
pub fn section_report(path: &Path) -> Result<Vec<(String, u64, String)>, String> {
    let data = read(path)?;
    let file = parse(&data, path)?;
    let mut rows = Vec::new();
    for section in file.sections() {
        let name = section.name().unwrap_or("<unnamed>").to_string();
        let size = section.size();
        let digest = section
            .data()
            .map(sha256_hex)
            .unwrap_or_else(|_| "<no data>".to_string());
        rows.push((name, size, digest));
    }
    Ok(rows)
}

/// Render two section reports side by side, keeping only differing rows.
pub fn section_diff(left: &[(String, u64, String)], right: &[(String, u64, String)]) -> String {
    let mut out = String::new();
    let names: Vec<&String> = left
        .iter()
        .map(|(name, _, _)| name)
        .chain(right.iter().map(|(name, _, _)| name))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        if !seen.insert(name.clone()) {
            continue;
        }
        let find = |rows: &[(String, u64, String)]| {
            rows.iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, size, digest)| (*size, digest.clone()))
        };
        let (l, r) = (find(left), find(right));
        if l != r {
            out.push_str(&format!("  {name}: {l:?} vs {r:?}\n"));
        }
    }
    if out.is_empty() {
        out.push_str("  (no section-level difference; header or padding drift)\n");
    }
    out
}

/// Undefined symbol names, sorted — what the artifact still needs at link
/// or load time.
pub fn undefined_symbols(path: &Path) -> Result<Vec<String>, String> {
    let data = read(path)?;
    let file = parse(&data, path)?;
    let mut names: Vec<String> = file
        .symbols()
        .filter(|symbol| symbol.is_undefined())
        .filter_map(|symbol| symbol.name().ok().map(str::to_string))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Defined, globally visible `mj_*`/`mjrt_*` names, sorted — the exported
/// runtime/program surface of an executable or object.
pub fn exported_runtime_surface(path: &Path) -> Result<Vec<String>, String> {
    let data = read(path)?;
    let file = parse(&data, path)?;
    let mut names: Vec<String> = file
        .symbols()
        .filter(|symbol| !symbol.is_undefined() && symbol.is_global())
        .filter_map(|symbol| symbol.name().ok().map(str::to_string))
        .filter(|name| name.starts_with("mj_") || name.starts_with("mjrt_"))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Load-bearing ELF facts for the artifact contract.
#[derive(Debug, PartialEq, Eq)]
pub struct ElfFacts {
    /// `EM_X86_64` expected on the supported target.
    pub machine: String,
    /// `ET_DYN` (a PIE on modern toolchains) or `ET_EXEC`/`ET_REL`.
    pub kind: String,
    /// Whether a `PT_GNU_STACK` header requests an executable stack.
    pub executable_stack: bool,
    /// `DT_NEEDED` entries (dynamic executables only), sorted.
    pub needed: Vec<String>,
}

/// Machine, object kind, stack policy, and dynamic dependencies.
pub fn elf_facts(path: &Path) -> Result<ElfFacts, String> {
    use object::elf;
    use object::read::elf::{ElfFile64, FileHeader, ProgramHeader};

    let data = read(path)?;
    let file: ElfFile64 = ElfFile64::parse(&*data)
        .map_err(|error| format!("{} is not ELF64: {error}", path.display()))?;
    let header = file.elf_header();
    let endian = file.endian();
    let machine = match header.e_machine(endian) {
        elf::EM_X86_64 => "x86-64".to_string(),
        other => format!("machine {other}"),
    };
    let kind = match header.e_type(endian) {
        elf::ET_REL => "relocatable".to_string(),
        elf::ET_EXEC => "executable".to_string(),
        elf::ET_DYN => "shared/pie".to_string(),
        other => format!("type {other}"),
    };
    let mut executable_stack = false;
    for segment in file.elf_program_headers() {
        if segment.p_type(endian) == elf::PT_GNU_STACK
            && segment.p_flags(endian).0 & elf::PF_X.0 != 0
        {
            executable_stack = true;
        }
    }
    // `object`'s generic import API covers DT_NEEDED for dynamic objects.
    let mut needed = Vec::new();
    let generic = parse(&data, path)?;
    if let Ok(imports) = generic.imports() {
        for import in imports {
            let Ok(import) = import else { continue };
            let library = String::from_utf8_lossy(import.library()).into_owned();
            // Symbol imports without a resolved library render empty.
            if !library.is_empty() {
                needed.push(library);
            }
        }
    }
    needed.sort();
    needed.dedup();
    Ok(ElfFacts {
        machine,
        kind,
        executable_stack,
        needed,
    })
}

/// Whether the file's raw bytes contain `needle` — the build-tree-path scan
/// for release/bundle artifacts (development builds legitimately embed
/// cargo paths in the runtime archive's DWARF and are exempt).
pub fn contains_bytes(path: &Path, needle: &[u8]) -> Result<bool, String> {
    let data = read(path)?;
    Ok(data.windows(needle.len()).any(|window| window == needle))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn parse<'a>(data: &'a [u8], path: &Path) -> Result<object::File<'a>, String> {
    object::File::parse(data)
        .map_err(|error| format!("cannot parse {} as an object: {error}", path.display()))
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
