# expect: the SIMD type must be floating point
# `Int(3.9)` binds upstream's exact-type `@implicit SIMD.__init__(FloatLiteral)`
# ahead of the `Intable` overload, and its float-dtype assert fails.
def main():
    print(Int(3.9))
