# expect: expected Int, found StringLiteral
# A keyword-bound infer-only type parameter fixes the parameter's type, so a
# runtime argument of another type must convert to it, as on the pin.
def f[T: Writable & ImplicitlyCopyable, //, n: Int](x: T) -> Int:
    return n


def main():
    print(f[T=Int, n=3]("a"))
