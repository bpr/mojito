# expect: invalid call to 'pop.call_llvm_intrinsic': op requires attribute 'hasSideEffects'
# `pop.call_llvm_intrinsic` requires its `hasSideEffects` attribute, as the
# pin's verifier does.
def main():
    var x = Int32(7)
    print(__mlir_op.`pop.call_llvm_intrinsic`[intrin="llvm.ctpop".value, _type=Int32](x))
