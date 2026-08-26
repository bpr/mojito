//! Source-level LLVM debug information (Stage 6, S6.3).
//!
//! pliron-llvm 0.17's converter drops pliron locations entirely, so debug
//! info is attached after conversion: the stamped LLVM IR text is reparsed
//! into our own `llvm_sys` context and annotated through the C DIBuilder
//! API, then written to bitcode directly — no pliron fork.
//!
//! Correlation is call-granular: pliron `CallOp`/`CallIntrinsicOp` are the
//! only operations that convert to LLVM `call` instructions, they never
//! constant-fold, and both sides emit them in block order — so the i-th
//! call instruction of a function corresponds to the i-th recorded call
//! location. A per-function count assertion guards that premise: on
//! mismatch the function degrades to a subprogram-only entry (never a
//! wrong line) and is reported, and the corpus test pins zero degradations.
//! Calls without a recorded source position (runtime helpers, the exe
//! wrapper's calls) get an artificial line-0 location, which also satisfies
//! the LLVM verifier's rule that calls inside a function with a
//! `DISubprogram` carry `!dbg`. Every mojito function lowers from a single
//! source file; a recorded call location in a different file than its
//! function's anchor also degrades rather than mislabeling.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::Path;

use pliron::builtin::op_interfaces::SymbolOpInterface;
use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron::linked_list::ContainsLinkedList;
use pliron::location::{Located, Location};
use pliron::op::Op;
use pliron::operation::Operation;

use super::lower::Locator;
use super::{DebugInfo, PlironError, PlironErrorKind};

/// The emission-time debug policy: the requested level paired with the
/// module's harvested facts.
#[derive(Clone, Copy)]
pub(super) struct DebugPolicy<'a> {
    pub(super) level: DebugInfo,
    pub(super) table: &'a DebugTable,
}

/// Per-function debug facts harvested from the pliron module after the
/// cleanup pipeline (passes may delete calls and unreachable blocks, so
/// collection must see the final IR).
pub(super) struct DebugTable {
    /// Source labels exactly as registered with the [`Locator`]; index is
    /// the file id used by [`FnDebug`].
    files: Vec<String>,
    /// Mangled symbol name → facts.
    functions: HashMap<String, FnDebug>,
}

struct FnDebug {
    /// Subprogram anchor: (file id, line) of the function's first located
    /// operation. `None` for fully synthetic functions.
    anchor: Option<(usize, u32)>,
    /// One entry per call instruction in emission order: located calls
    /// carry (file id, line, column); synthetic calls carry `None`.
    calls: Vec<Option<(usize, u32, u32)>>,
}

impl DebugTable {
    /// Harvest function anchors and per-call locations from the lowered,
    /// cleaned module.
    pub(super) fn collect(ctx: &Context, module: ModuleOp, locator: &Locator) -> DebugTable {
        let files: Vec<String> = locator.source_labels().map(str::to_string).collect();
        let source_ids: HashMap<pliron::location::Source, usize> = locator
            .sources()
            .enumerate()
            .map(|(index, source)| (source, index))
            .collect();
        let locate = |op: &Operation| -> Option<(usize, u32, u32)> {
            match op.loc() {
                Location::SrcPos { src, pos } => source_ids.get(&src).map(|&file| {
                    (
                        file,
                        u32::try_from(pos.line).unwrap_or(0),
                        u32::try_from(pos.column).unwrap_or(0),
                    )
                }),
                _ => None,
            }
        };

        let mut functions = HashMap::new();
        let module_op = module.get_operation();
        for region in module_op.deref(ctx).regions() {
            for block in region.deref(ctx).iter(ctx) {
                for top in block.deref(ctx).iter(ctx) {
                    let Some(func) = Operation::get_op::<pliron_llvm::ops::FuncOp>(top, ctx) else {
                        continue;
                    };
                    let name = func.get_symbol_name(ctx).to_string();
                    let mut anchor = None;
                    let mut calls = Vec::new();
                    for func_region in top.deref(ctx).regions() {
                        for func_block in func_region.deref(ctx).iter(ctx) {
                            for op in func_block.deref(ctx).iter(ctx) {
                                let located = locate(&op.deref(ctx));
                                if anchor.is_none()
                                    && let Some((file, line, _)) = located
                                {
                                    anchor = Some((file, line));
                                }
                                if Operation::is_op::<pliron_llvm::ops::CallOp>(op, ctx)
                                    || Operation::is_op::<pliron_llvm::ops::CallIntrinsicOp>(
                                        op, ctx,
                                    )
                                {
                                    calls.push(located);
                                }
                            }
                        }
                    }
                    functions.insert(name, FnDebug { anchor, calls });
                }
            }
        }
        DebugTable { files, functions }
    }
}

/// Parse the stamped LLVM IR text, attach line-level debug information from
/// `table`, verify, and write bitcode to `path`. Returns the names of
/// functions that degraded to subprogram-only entries (call-count or
/// cross-file mismatch); the corpus test pins this to empty.
pub(super) fn write_bitcode_with_debug(
    ir_text: &str,
    table: &DebugTable,
    path: &Path,
) -> Result<Vec<String>, PlironError> {
    use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
    use llvm_sys::bit_writer::LLVMWriteBitcodeToFile;
    use llvm_sys::core::*;
    use llvm_sys::debuginfo::*;
    use llvm_sys::ir_reader::LLVMParseIRInContext2;
    use llvm_sys::prelude::*;

    let path_str = path
        .to_str()
        .ok_or_else(|| debug_error(format!("non-UTF-8 output path {}", path.display())))?;
    let mut degraded = Vec::new();

    unsafe {
        let llctx = LLVMContextCreate();
        let buffer = LLVMCreateMemoryBufferWithMemoryRangeCopy(
            ir_text.as_ptr().cast(),
            ir_text.len(),
            c"mojito-debug-attach".as_ptr(),
        );
        let mut module: LLVMModuleRef = std::ptr::null_mut();
        let mut message: *mut std::ffi::c_char = std::ptr::null_mut();
        let parse_failed = LLVMParseIRInContext2(llctx, buffer, &mut module, &mut message) != 0;
        // Unlike its deprecated predecessor, `2` never consumes the buffer.
        LLVMDisposeMemoryBuffer(buffer);
        if parse_failed {
            let text = CStr::from_ptr(message).to_string_lossy().into_owned();
            LLVMDisposeMessage(message);
            LLVMContextDispose(llctx);
            return Err(debug_error(format!("debug-attach reparse failed: {text}")));
        }

        let dibuilder = LLVMCreateDIBuilder(module);
        let empty = c"";
        let difiles: Vec<LLVMMetadataRef> = table
            .files
            .iter()
            .map(|label| {
                let label = dwarf_file_label(label);
                LLVMDIBuilderCreateFile(
                    dibuilder,
                    label.as_ptr().cast(),
                    label.len(),
                    empty.as_ptr(),
                    0,
                )
            })
            .collect();
        // A synthetic file for functions with no source anchor (the exe
        // wrapper) keeps every subprogram well-formed.
        let synthetic = {
            let label = "<mojito>";
            LLVMDIBuilderCreateFile(
                dibuilder,
                label.as_ptr().cast(),
                label.len(),
                empty.as_ptr(),
                0,
            )
        };
        let producer = "mojito";
        let cu_file = difiles.first().copied().unwrap_or(synthetic);
        let compile_unit = LLVMDIBuilderCreateCompileUnit(
            dibuilder,
            LLVMDWARFSourceLanguage::LLVMDWARFSourceLanguageC,
            cu_file,
            producer.as_ptr().cast(),
            producer.len(),
            0,
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            0,
            LLVMDWARFEmissionKind::LLVMDWARFEmissionKindFull,
            0,
            0,
            0,
            empty.as_ptr(),
            0,
            empty.as_ptr(),
            0,
        );
        let _ = compile_unit;

        let mut function = LLVMGetFirstFunction(module);
        while !function.is_null() {
            if LLVMCountBasicBlocks(function) > 0 {
                let mut name_len = 0usize;
                let name_ptr = LLVMGetValueName2(function, &mut name_len);
                let name = std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    name_ptr.cast(),
                    name_len,
                ))
                .to_string();

                // Count the function's call instructions first: correlation
                // only holds when it matches the recorded call list.
                let mut call_count = 0usize;
                let mut block = LLVMGetFirstBasicBlock(function);
                while !block.is_null() {
                    let mut inst = LLVMGetFirstInstruction(block);
                    while !inst.is_null() {
                        if !LLVMIsACallInst(inst).is_null() {
                            call_count += 1;
                        }
                        inst = LLVMGetNextInstruction(inst);
                    }
                    block = LLVMGetNextBasicBlock(block);
                }

                let entry = table.functions.get(&name);
                let usable = match entry {
                    Some(fnd) if fnd.calls.len() == call_count => {
                        // Reject cross-file call locations outright: a
                        // wrong file is worse than no location.
                        let same_file = fnd.anchor.map(|(file, _)| file);
                        if fnd
                            .calls
                            .iter()
                            .flatten()
                            .all(|(file, _, _)| Some(*file) == same_file)
                        {
                            Some(fnd)
                        } else {
                            degraded.push(name.clone());
                            None
                        }
                    }
                    Some(_) => {
                        degraded.push(name.clone());
                        None
                    }
                    // Not in the table at all (the exe wrapper): artificial
                    // subprogram, no degradation.
                    None => None,
                };

                let (file, line, flags) = match usable.and_then(|fnd| fnd.anchor) {
                    Some((file, line)) => (difiles[file], line, LLVMDIFlagZero),
                    None => (synthetic, 0, LLVMDIFlagArtificial),
                };
                let subroutine_ty = LLVMDIBuilderCreateSubroutineType(
                    dibuilder,
                    file,
                    std::ptr::null_mut(),
                    0,
                    LLVMDIFlagZero,
                );
                let subprogram = LLVMDIBuilderCreateFunction(
                    dibuilder,
                    file,
                    name.as_ptr().cast(),
                    name.len(),
                    name.as_ptr().cast(),
                    name.len(),
                    file,
                    line,
                    subroutine_ty,
                    0,
                    1,
                    line,
                    flags,
                    0,
                );
                LLVMSetSubprogram(function, subprogram);

                // Second walk: attach a location to every call instruction.
                let mut call_index = 0usize;
                let mut block = LLVMGetFirstBasicBlock(function);
                while !block.is_null() {
                    let mut inst = LLVMGetFirstInstruction(block);
                    while !inst.is_null() {
                        if !LLVMIsACallInst(inst).is_null() {
                            let located =
                                usable.and_then(|fnd| fnd.calls.get(call_index).copied().flatten());
                            let (line, column) = match located {
                                Some((_, line, column)) => (line, column),
                                None => (0, 0),
                            };
                            let location = LLVMDIBuilderCreateDebugLocation(
                                llctx,
                                line,
                                column,
                                subprogram,
                                std::ptr::null_mut(),
                            );
                            LLVMInstructionSetDebugLoc(inst, location);
                            call_index += 1;
                        }
                        inst = LLVMGetNextInstruction(inst);
                    }
                    block = LLVMGetNextBasicBlock(block);
                }
            }
            function = LLVMGetNextFunction(function);
        }

        LLVMDIBuilderFinalize(dibuilder);

        // "Debug Info Version" is required for the metadata to survive;
        // Dwarf 5 matches the pinned toolchain's default.
        let i32_ty = LLVMInt32TypeInContext(llctx);
        for (key, value) in [(c"Debug Info Version", 3u64), (c"Dwarf Version", 5u64)] {
            LLVMAddModuleFlag(
                module,
                llvm_sys::LLVMModuleFlagBehavior::LLVMModuleFlagBehaviorWarning,
                key.as_ptr(),
                key.count_bytes(),
                LLVMValueAsMetadata(LLVMConstInt(i32_ty, value, 0)),
            );
        }

        let mut verify_message: *mut std::ffi::c_char = std::ptr::null_mut();
        let broken = LLVMVerifyModule(
            module,
            LLVMVerifierFailureAction::LLVMReturnStatusAction,
            &mut verify_message,
        );
        if broken != 0 {
            let text = CStr::from_ptr(verify_message)
                .to_string_lossy()
                .into_owned();
            LLVMDisposeMessage(verify_message);
            LLVMDisposeDIBuilder(dibuilder);
            LLVMDisposeModule(module);
            LLVMContextDispose(llctx);
            return Err(debug_error(format!(
                "module verification failed after debug attach: {text}"
            )));
        }
        if !verify_message.is_null() {
            LLVMDisposeMessage(verify_message);
        }

        let path_c = CString::new(path_str)
            .map_err(|_| debug_error(format!("NUL in output path {path_str}")))?;
        let wrote = LLVMWriteBitcodeToFile(module, path_c.as_ptr());
        LLVMDisposeDIBuilder(dibuilder);
        LLVMDisposeModule(module);
        LLVMContextDispose(llctx);
        if wrote != 0 {
            return Err(debug_error(format!(
                "bitcode emission after debug attach failed for {path_str}"
            )));
        }
    }
    Ok(degraded)
}

/// The name a source label embeds as its `DIFile`: relative labels verbatim,
/// but an absolute label degrades to its file name — emitted artifacts must
/// stay byte-identical across build directories (the reproducibility
/// contract), and an absolute path baked into DWARF would break that.
/// Debuggers resolve the file through their source search paths instead.
fn dwarf_file_label(label: &str) -> &str {
    let path = Path::new(label);
    if !path.is_absolute() {
        return label;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(label)
}

fn debug_error(message: String) -> PlironError {
    PlironError {
        function: None,
        kind: PlironErrorKind::Emit(message),
        location: None,
    }
}

#[cfg(test)]
mod tests {
    use super::dwarf_file_label;

    #[test]
    fn pliron_dwarf_labels_never_embed_absolute_paths() {
        assert_eq!(
            dwarf_file_label("examples/mandel.mojo"),
            "examples/mandel.mojo"
        );
        assert_eq!(dwarf_file_label("-"), "-");
        assert_eq!(dwarf_file_label("/tmp/build/prog.mojo"), "prog.mojo");
        assert_eq!(dwarf_file_label("/"), "/");
    }
}
