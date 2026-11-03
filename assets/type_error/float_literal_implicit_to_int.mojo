# expect: the SIMD type must be floating point
# `var x: Int = 2.5` converts through upstream's `@implicit
# SIMD.__init__(FloatLiteral)`, whose float-dtype assert fails at `Int`.
def main():
    var x: Int = 2.5
    print(x)
