# expect: op attribute 'intrin' failed to satisfy constraint: TypedAttr instance String type
# `pop.call_llvm_intrinsic`'s `intrin` is a `!kgen.string`; the pin's
# verifier rejects a `StringLiteral` in its place.
def main():
    var x = Int32(7)
    print(
        __mlir_op.`pop.call_llvm_intrinsic`[
            intrin="llvm.ctpop", _type=Int32, hasSideEffects=False.__mlir_i1__()
        ](x)
    )
