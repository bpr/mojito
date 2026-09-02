//! Per-instruction result/operand register enumeration.

use super::*;

/// The result registers an instruction defines (call/operation destinations
/// and loan/consumption markers).
pub(crate) fn instruction_result_regs(instruction: &MirInstr, out: &mut Vec<Reg>) {
    match instruction {
        MirInstr::MakeRef { dest, .. }
        | MirInstr::ReadRef { dest, .. }
        | MirInstr::CopyValue { dest, .. }
        | MirInstr::Const { dest, .. }
        | MirInstr::SizeOf { dest, .. }
        | MirInstr::ConstructTypeParam { dest, .. }
        | MirInstr::MaterializeLiteral { dest, .. }
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
        | MirInstr::Slice { dest, .. }
        | MirInstr::MultiIndex { dest, .. }
        | MirInstr::LoadPlace { dest, .. }
        | MirInstr::MakeTuple { dest, .. }
        | MirInstr::MakeVariant { dest, .. }
        | MirInstr::MakeSimd { dest, .. }
        | MirInstr::SimdCast { dest, .. }
        | MirInstr::SimdShuffle { dest, .. }
        | MirInstr::MakeClosure { dest, .. }
        | MirInstr::VariantIs { dest, .. }
        | MirInstr::VariantGet { dest, .. }
        | MirInstr::VariantSet { dest, .. }
        | MirInstr::VariantSetInitWith { dest, .. }
        | MirInstr::VariantTake { dest, .. }
        | MirInstr::VariantDeinitWith { dest, .. }
        | MirInstr::VariantReplace { dest, .. }
        | MirInstr::HasNext { dest, .. }
        | MirInstr::Next { dest, .. } => out.push(*dest),
        MirInstr::TryNext { dest, yielded, .. } => {
            out.push(*dest);
            out.push(*yielded);
        }
        MirInstr::EstablishLoans { marker, .. }
        | MirInstr::InvalidateInteriors { marker, .. }
        | MirInstr::ConsumePlace { marker, .. } => out.push(*marker),
        _ => {}
    }
}

/// The registers an instruction reads (operands, arguments, stored values, and
/// place index registers). `Try` sub-regions are walked separately.
pub(crate) fn instruction_operand_regs(instruction: &MirInstr, out: &mut Vec<Reg>) {
    let place = |p: &MirPlace, out: &mut Vec<Reg>| {
        for projection in &p.proj {
            if let Proj::Index(register) = projection {
                out.push(*register);
            }
        }
    };
    match instruction {
        MirInstr::EstablishLoans { loans, .. } => {
            for loan in loans {
                place(&loan.place, out);
            }
        }
        MirInstr::ConsumePlace { place: p, .. }
        | MirInstr::MakeRef { place: p, .. }
        | MirInstr::MovePlace { place: p, .. }
        | MirInstr::LoadPlace { place: p, .. } => place(p, out),
        MirInstr::ReadRef { reference, .. } => out.push(*reference),
        MirInstr::CopyValue { value, .. } => out.push(*value),
        MirInstr::WriteRef { reference, value } => out.extend([*reference, *value]),
        MirInstr::MaterializeLiteral { value, .. } => out.push(*value),
        MirInstr::UnOp { a, .. } => out.push(*a),
        MirInstr::BinOp { a, b, .. } => out.extend([*a, *b]),
        MirInstr::Store { place: p, src } => {
            place(p, out);
            out.push(*src);
        }
        MirInstr::StoreRef {
            place: p,
            reference,
        } => {
            place(p, out);
            out.push(*reference);
        }
        MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            call,
            ..
        } => {
            out.push(*receiver);
            if let Some(receiver_place) = receiver_place {
                place(receiver_place, out);
            }
            for argument in args {
                subscript_arg_regs(argument, out);
            }
            for argument_place in arg_places.iter().flatten() {
                place(argument_place, out);
            }
            out.push(*value);
            if let Some(value_place) = value_place {
                place(value_place, out);
            }
            out.extend(
                call.param_arg_regs
                    .iter()
                    .filter_map(|argument| argument.value),
            );
        }
        MirInstr::Call {
            args,
            kwargs,
            arg_places,
            kwarg_places,
            param_arg_regs,
            ..
        } => {
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            for p in arg_places.iter().flatten() {
                place(p, out);
            }
            for p in kwarg_places.iter().flatten() {
                place(p, out);
            }
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            param_arg_regs,
            callee_place,
            arg_places,
            kwarg_places,
            ..
        } => {
            out.push(*callee);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
            for p in callee_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
            {
                place(p, out);
            }
        }
        MirInstr::MethodCall {
            recv,
            args,
            kwargs,
            param_arg_regs,
            recv_place,
            arg_places,
            kwarg_places,
            ..
        } => {
            out.push(*recv);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
            for p in recv_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
            {
                place(p, out);
            }
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
            base,
            index,
            base_place,
            index_place,
            call,
            ..
        } => {
            out.extend([*base, *index]);
            if let Some(base_place) = base_place {
                place(base_place, out);
            }
            if let Some(index_place) = index_place {
                place(index_place, out);
            }
            if let Some(call) = call {
                out.extend(
                    call.param_arg_regs
                        .iter()
                        .filter_map(|argument| argument.value),
                );
            }
        }
        MirInstr::Slice {
            object,
            lower,
            upper,
            step,
            object_place,
            arg_places,
            call,
            ..
        } => {
            out.push(*object);
            out.extend([lower, upper, step].into_iter().flatten().copied());
            if let Some(object_place) = object_place {
                place(object_place, out);
            }
            for argument_place in arg_places.iter().flatten() {
                place(argument_place, out);
            }
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
            object_place,
            arg_places,
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
            if let Some(object_place) = object_place {
                place(object_place, out);
            }
            for argument_place in arg_places.iter().flatten() {
                place(argument_place, out);
            }
            if let Some(call) = call {
                out.extend(
                    call.param_arg_regs
                        .iter()
                        .filter_map(|argument| argument.value),
                );
            }
        }
        MirInstr::MakeTuple { elems, .. } | MirInstr::MakeSimd { elems, .. } => {
            out.extend(elems.iter().copied())
        }
        MirInstr::SimdCast { value, .. } | MirInstr::SimdShuffle { value, .. } => out.push(*value),
        MirInstr::MakeVariant { value, .. } => out.push(*value),
        MirInstr::MakeClosure { captures, .. } => {
            for capture in captures {
                place(&capture.place, out);
            }
        }
        MirInstr::VariantIs { variant, .. } | MirInstr::VariantGet { variant, .. } => {
            out.push(*variant)
        }
        MirInstr::VariantTake { variant, .. } => out.push(*variant),
        MirInstr::VariantDeinitWith {
            variant, handler, ..
        } => {
            out.push(*variant);
            out.push(*handler);
        }
        MirInstr::VariantSetInitWith {
            place: p, factory, ..
        } => {
            place(p, out);
            out.push(*factory);
        }
        MirInstr::VariantSet {
            place: p, value, ..
        } => {
            place(p, out);
            out.push(*value);
        }
        MirInstr::VariantReplace {
            place: p, value, ..
        } => {
            place(p, out);
            out.push(*value);
        }
        MirInstr::Raise { src } => out.push(*src),
        MirInstr::Drop { reg } => out.push(*reg),
        MirInstr::DefVar { src, .. } => out.push(*src),
        MirInstr::InvalidateInteriors { .. }
        | MirInstr::Const { .. }
        | MirInstr::SizeOf { .. }
        | MirInstr::ConstructTypeParam { .. }
        | MirInstr::UseVar { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::DropVar { .. }
        | MirInstr::ConsumeVar { .. }
        | MirInstr::GetIter { .. }
        | MirInstr::HasNext { .. }
        | MirInstr::Next { .. }
        | MirInstr::TryNext { .. }
        | MirInstr::Unsupported(_)
        | MirInstr::Try { .. } => {}
    }
}
