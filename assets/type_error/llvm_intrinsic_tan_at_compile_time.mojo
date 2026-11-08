# expect: LLVM could not constant fold intrinsic: llvm.tan
# The comptime interpreter calls only an LLVM intrinsic LLVM's constant
# folder folds, and it does not fold `llvm.tan`, though it folds `llvm.sin`.
from std.sys import llvm_intrinsic


def main():
    comptime t = llvm_intrinsic["llvm.tan", Float64, has_side_effect=False](
        Float64(0.5)
    )
    print(t)
