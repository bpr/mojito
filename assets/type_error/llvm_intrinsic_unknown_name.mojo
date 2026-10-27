# expect: the LLVM intrinsic 'llvm.no.such.thing' is outside the subset
# A reached `llvm_intrinsic` call names an intrinsic LLVM does not know,
# which the pin rejects when it translates the instance.
from std.sys import llvm_intrinsic


def main():
    print(llvm_intrinsic["llvm.no.such.thing", Int32, has_side_effect=False](Int32(1)))
