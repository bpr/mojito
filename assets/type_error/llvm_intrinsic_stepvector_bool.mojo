# expect: stepvector only supported for vectors of integers with a bitwidth of at least 8
# `llvm.stepvector` over `bool` lanes, which LLVM's verifier rejects: its
# lanes are integers of at least eight bits.
from std.sys import llvm_intrinsic


def main():
    print(llvm_intrinsic["llvm.stepvector", SIMD[DType.bool, 4], has_side_effect=False]())
