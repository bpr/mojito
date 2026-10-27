# expect: bswap must be an even number of bytes
# `llvm.bswap` takes lanes of an even number of bytes; the pin rejects a
# reached call over `Int8` when it translates the instance.
from std.sys import llvm_intrinsic


def main():
    print(llvm_intrinsic["llvm.bswap", Int8, has_side_effect=False](Int8(1)))
