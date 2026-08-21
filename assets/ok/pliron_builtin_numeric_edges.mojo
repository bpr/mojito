# The numeric builtins' native edges, VM-exact: abs wraps on Int
# (abs(i64::MIN) == i64::MIN), passes UInt through unchanged, and is fabs
# on Float64; min/max pick left on ties; round is ties-away-from-zero on
# its (checker-required) Float64 argument.
def main():
    print(abs(-5), abs(5), abs(-3.5), abs(2.5))
    var u: UInt = 7
    print(abs(u))
    print(abs(min(-4, -2)), max(-4, -2))
    print(min(2.5, 1.5), max(2.5, 1.5))
    var big: Int = -9223372036854775807 - 1
    print(abs(big))
    print(round(2.5), round(-2.5), round(2.4), round(-2.4))
