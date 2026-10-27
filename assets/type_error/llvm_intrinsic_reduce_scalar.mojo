# expect: the intrinsic signature of 'llvm.vector.reduce.smax' does not match
# An `llvm.vector.reduce` intrinsic takes a vector; the pin rejects a
# reached call over a scalar when it translates the instance.
from std.sys import llvm_intrinsic


def main():
    print(llvm_intrinsic["llvm.vector.reduce.smax", Int32, has_side_effect=False](Int32(5)))
