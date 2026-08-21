# The scalar rounding intrinsics behind std.math, native vs VM: the
# __floor__/__ceil__/__trunc__ dunders are identity on integers and LLVM's
# exact f64 rounding on Float64; __ceildiv__ rounds toward +inf on every
# operand kind (Int through the negated flooring division, UInt through the
# remainder carry, Float64 as ceil(a / b)).
from std.math import floor, ceil, trunc, ceildiv

def main():
    print(floor(3.7), ceil(3.2), trunc(-3.7), trunc(3.7))
    print(floor(-3.7), ceil(-3.2))
    print(floor(5), ceil(5), trunc(5))
    print(ceildiv(7, 2), ceildiv(-7, 2), ceildiv(7, -2), ceildiv(8, 2))
    print(ceildiv(7.0, 2.0), ceildiv(-7.0, 2.0))
    var u: UInt = 7
    var d: UInt = 2
    print(ceildiv(u, d))
