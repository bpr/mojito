# `assume` tells the optimizer a condition holds through `llvm.assume`,
# which computes nothing, as at the pin.
from std.sys.intrinsics import assume


def main():
    var n = 3
    assume(n == 3)
    print(n)
