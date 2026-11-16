//! Per-instruction result/operand register enumeration.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// The result registers an instruction defines (call/operation destinations,
/// a raising instruction's error flag and error, and loan/consumption
/// markers).
pub fn instruction_result_regs(instruction: &MirInstr, out: &mut Vec<Reg>) {
    match instruction {
        MirInstr::MakeRef { dest, .. }
        | MirInstr::ReadRef { dest, .. }
        | MirInstr::CopyValue { dest, .. }
        | MirInstr::Rebind { dest, .. }
        | MirInstr::Const { dest, .. }
        | MirInstr::ParamListAddress { dest, .. }
        | MirInstr::IsRunInComptimeInterpreter { dest }
        | MirInstr::SourceLoc { dest, .. }
        | MirInstr::ConstructTypeParam { dest, .. }
        | MirInstr::ConstructType { dest, .. }
        | MirInstr::MaterializeLiteral { dest, .. }
        | MirInstr::ScalarCast { dest, .. }
        | MirInstr::BuiltinCast { dest, .. }
        | MirInstr::PointerConvert { dest, .. }
        | MirInstr::PointerBitcast { dest, .. }
        | MirInstr::Pop { dest, .. }
        | MirInstr::CallLlvmIntrinsic { dest, .. }
        | MirInstr::StoreToMem { dest, .. }
        | MirInstr::UseVar { dest, .. }
        | MirInstr::MovePlace { dest, .. }
        | MirInstr::UnOp { dest, .. }
        | MirInstr::BinOp { dest, .. }
        | MirInstr::Call { dest, .. }
        | MirInstr::CallIndirect { dest, .. }
        | MirInstr::MethodCall { dest, .. }
        | MirInstr::PointerStorageTake { dest, .. }
        | MirInstr::PointerStorageDestroy { dest, .. }
        | MirInstr::UninitStorage { dest, .. }
        | MirInstr::UninitStorageTake { dest, .. }
        | MirInstr::UninitStorageDestroy { dest, .. }
        | MirInstr::GetField { dest, .. }
        | MirInstr::Index { dest, .. }
        | MirInstr::MultiIndex { dest, .. }
        | MirInstr::LoadPlace { dest, .. }
        | MirInstr::MakeTuple { dest, .. }
        | MirInstr::MakeStruct { dest, .. }
        | MirInstr::MakeArray { dest, .. }
        | MirInstr::MakeRefPack { dest, .. }
        | MirInstr::LoadIndirect { dest, .. }
        | MirInstr::ArrayRepeat { dest, .. }
        | MirInstr::ArrayGet { dest, .. }
        | MirInstr::ArrayReplace { dest, .. }
        | MirInstr::ArrayGep { dest, .. }
        | MirInstr::MakeVariant { dest, .. }
        | MirInstr::MakeSimd { dest, .. }
        | MirInstr::SimdCast { dest, .. }
        | MirInstr::SimdBitcast { dest, .. }
        | MirInstr::SimdShuffle { dest, .. }
        | MirInstr::Select { dest, .. }
        | MirInstr::SimdSelect { dest, .. }
        | MirInstr::SimdExtract { dest, .. }
        | MirInstr::SimdInsert { dest, .. }
        | MirInstr::SimdReduce { dest, .. }
        | MirInstr::MakeClosure { dest, .. }
        | MirInstr::VariantIs { dest, .. }
        | MirInstr::VariantTag { dest, .. }
        | MirInstr::VariantGet { dest, .. }
        | MirInstr::VariantSet { dest, .. }
        | MirInstr::VariantTake { dest, .. }
        | MirInstr::VariantReplace { dest, .. }
        | MirInstr::HasNext { dest, .. }
        | MirInstr::Next { dest, .. } => out.push(*dest),
        MirInstr::EstablishLoans { marker, .. } | MirInstr::InvalidateInteriors { marker, .. } => {
            out.push(*marker);
        }
        MirInstr::If {
            results,
            elifs,
            else_args,
            ..
        } => out.extend(
            results
                .iter()
                .chain(
                    elifs
                        .iter()
                        .flat_map(|elif| elif.cond_args.iter().chain(&elif.then_args)),
                )
                .chain(else_args)
                .copied(),
        ),
        MirInstr::Match {
            results,
            cases,
            else_args,
            ..
        } => out.extend(
            results
                .iter()
                .chain(cases.iter().flat_map(|case| &case.args))
                .chain(else_args)
                .copied(),
        ),
        MirInstr::Try {
            results,
            handler_args,
            else_args,
            ..
        } => out.extend(results.iter().chain(handler_args).chain(else_args).copied()),
        MirInstr::ComptimeIf { results, .. } => out.extend(results.iter().copied()),
        MirInstr::Loop { results, args, .. } | MirInstr::ComptimeFor { results, args, .. } => {
            out.extend(results.iter().chain(args).copied());
        }
        _ => {}
    }
    if let Some(raises) = instruction.raise_edge() {
        out.push(raises.flag);
    }
}

/// The registers an instruction reads (operands, arguments, stored values, and
/// place index registers). Region ops' blocks are walked separately.
pub fn instruction_operand_regs(instruction: &MirInstr, out: &mut Vec<Reg>) {
    match instruction {
        MirInstr::EstablishLoans { loans, .. } => {
            for loan in loans {
                index_regs(&loan.place, out);
            }
        }
        MirInstr::DropPlace { place: p, .. }
        | MirInstr::MarkInitialized { place: p }
        | MirInstr::MarkDestroyed { place: p }
        | MirInstr::MakeRef { place: p, .. }
        | MirInstr::MovePlace { place: p, .. }
        | MirInstr::LoadPlace { place: p, .. } => index_regs(p, out),
        MirInstr::CopyValue { value, .. }
        | MirInstr::ReadRef {
            reference: value, ..
        }
        | MirInstr::LoadIndirect { pack: value, .. }
        | MirInstr::Rebind { value, .. }
        | MirInstr::SimdReduce { value, .. }
        | MirInstr::MakeVariant { value, .. }
        | MirInstr::ArrayRepeat { value, .. }
        | MirInstr::ArrayGet { array: value, .. }
        | MirInstr::MaterializeLiteral { value, .. }
        | MirInstr::ScalarCast { value, .. }
        | MirInstr::BuiltinCast { value, .. }
        | MirInstr::PointerConvert { value, .. }
        | MirInstr::PointerBitcast { value, .. }
        | MirInstr::StoreToMem { src: value, .. } => out.push(*value),
        MirInstr::WriteRef {
            reference: array,
            value,
        }
        | MirInstr::ArrayReplace { array, value, .. } => out.extend([*array, *value]),
        MirInstr::ArrayGep { array, index, .. } => out.extend([*array, *index]),
        MirInstr::UnOp { a, .. } => out.push(*a),
        MirInstr::BinOp { a, b, .. } => out.extend([*a, *b]),
        MirInstr::Store { place: p, src } => {
            index_regs(p, out);
            out.push(*src);
        }
        MirInstr::StoreRef {
            place: p,
            reference,
        } => {
            index_regs(p, out);
            out.push(*reference);
        }
        MirInstr::MultiSet {
            receiver,
            args,
            value,
            call,
            ..
        } => {
            out.push(*receiver);
            for argument in args {
                subscript_arg_regs(argument, out);
            }
            out.push(*value);
            out.extend(
                call.param_arg_regs
                    .iter()
                    .filter_map(|argument| argument.value),
            );
        }
        MirInstr::Call {
            args,
            kwargs,
            param_arg_regs,
            ..
        } => {
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            param_arg_regs,
            ..
        } => {
            out.push(*callee);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
        }
        MirInstr::MethodCall {
            recv,
            args,
            kwargs,
            param_arg_regs,
            ..
        } => {
            out.push(*recv);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
        }
        MirInstr::PointerStorageTake { pointer, index, .. }
        | MirInstr::PointerStorageDestroy { pointer, index, .. } => {
            out.extend([*pointer, *index]);
        }
        MirInstr::UninitStorage { init, .. } => out.extend(init.iter().copied()),
        MirInstr::UninitStorageTake { storage, .. }
        | MirInstr::UninitStorageDestroy { storage, .. } => out.push(*storage),
        MirInstr::GetField { base, .. } => out.push(*base),
        MirInstr::Index {
            base, index, call, ..
        } => {
            out.extend([*base, *index]);
            if let Some(call) = call {
                out.extend(
                    call.param_arg_regs
                        .iter()
                        .filter_map(|argument| argument.value),
                );
            }
        }
        MirInstr::MultiIndex {
            object,
            args,
            kwargs,
            call,
            ..
        } => {
            out.push(*object);
            for argument in args {
                subscript_arg_regs(argument, out);
            }
            for (_, argument) in kwargs {
                subscript_arg_regs(argument, out);
            }
            if let Some(call) = call {
                out.extend(
                    call.param_arg_regs
                        .iter()
                        .filter_map(|argument| argument.value),
                );
            }
        }
        MirInstr::MakeTuple { elems, .. }
        | MirInstr::MakeSimd { elems, .. }
        | MirInstr::Pop {
            operands: elems, ..
        }
        | MirInstr::CallLlvmIntrinsic { args: elems, .. }
        | MirInstr::MakeArray { elems, .. }
        | MirInstr::MakeRefPack { elems, .. }
        | MirInstr::MakeStruct { fields: elems, .. } => {
            out.extend(elems.iter().copied());
        }
        MirInstr::SimdCast { value, .. } | MirInstr::SimdBitcast { value, .. } => out.push(*value),
        MirInstr::SimdShuffle { value, other, .. } => {
            out.push(*value);
            out.extend(*other);
        }
        MirInstr::Select {
            condition: mask,
            on_true,
            on_false,
            ..
        }
        | MirInstr::SimdSelect {
            mask,
            on_true,
            on_false,
            ..
        } => out.extend([*mask, *on_true, *on_false]),
        MirInstr::SimdExtract { value, index, .. } => out.extend([*value, *index]),
        MirInstr::SimdInsert {
            value,
            element,
            index,
            ..
        } => out.extend([*value, *element, *index]),
        MirInstr::MakeClosure { captures, .. } => {
            for capture in captures {
                index_regs(&capture.place, out);
            }
        }
        MirInstr::VariantIs { variant, .. }
        | MirInstr::VariantTag { variant, .. }
        | MirInstr::VariantGet { variant, .. }
        | MirInstr::VariantTake { variant, .. } => out.push(*variant),
        MirInstr::VariantSet {
            place: p, value, ..
        }
        | MirInstr::VariantReplace {
            place: p, value, ..
        } => {
            index_regs(p, out);
            out.push(*value);
        }
        MirInstr::ConstructTypeParam {
            kwargs, element, ..
        } => {
            out.extend(element.iter().filter_map(|argument| argument.value));
            out.extend(kwargs.iter().map(|(_, register)| *register));
        }
        MirInstr::If { cond, .. } => out.push(*cond),
        MirInstr::Loop { operands, .. } | MirInstr::ComptimeFor { operands, .. } => {
            out.extend(operands.iter().copied());
        }
        MirInstr::Drop { reg, .. } => out.push(*reg),
        MirInstr::DefVar { src, .. } => out.push(*src),
        MirInstr::InvalidateInteriors { .. }
        | MirInstr::Const { .. }
        | MirInstr::ParamListAddress { .. }
        | MirInstr::IsRunInComptimeInterpreter { .. }
        | MirInstr::SourceLoc { .. }
        | MirInstr::ConstructType { .. }
        | MirInstr::UseVar { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::DeclareVar { .. }
        | MirInstr::DropVar { .. }
        | MirInstr::ConsumeVar { .. }
        | MirInstr::GetIter { .. }
        | MirInstr::HasNext { .. }
        | MirInstr::Next { .. }
        | MirInstr::Unsupported(_)
        | MirInstr::Try { .. }
        | MirInstr::ComptimeIf { .. }
        | MirInstr::ComptimeAssert { .. }
        | MirInstr::Match { .. } => {}
    }
}

/// The registers indexing `p`'s projections.
fn index_regs(p: &MirPlace, out: &mut Vec<Reg>) {
    for projection in &p.proj {
        if let Proj::Index(register) = projection {
            out.push(*register);
        }
    }
}
