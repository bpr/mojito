# `expect`, `likely`, and `unlikely` give the optimizer a value's most
# probable value through `llvm.expect`, which gives the value itself, at
# run time, and at compile time, where `expect` returns the value without
# calling the intrinsic the comptime interpreter does not fold.
from std.sys.intrinsics import expect, likely, unlikely


def classify(n: Int) -> String:
    if likely(n > 0):
        return "positive"
    if unlikely(n == 0):
        return "zero"
    return "negative"


def main():
    print(classify(3), classify(0), classify(-2))
    var x = Int32(7)
    print(expect[Int32(7)](x), expect[UInt8(1)](UInt8(200)))
    print(expect[True](x == 7), unlikely(x == 7))
    comptime folded = expect[4](4) + Int(likely(True))
    print(folded)
