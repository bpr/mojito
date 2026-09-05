# expect: no constructor overload matches
# `List[Int](1, 2)` — an explicit type argument with positional elements —
# matches no List constructor upstream either (the variadic element
# constructor is inferred, `List(1, 2)`); both compilers reject it.
def main():
    var xs = List[Int](1, 2)
    print(len(xs))
