# A `def(...)` bound binds from a function name or a closure: a run-time
# local holding a function has a thin function type, which does not
# conform to the bound, as at the pin.
# expect: value passed to 'f' cannot be converted from 'def(Int) thin -> Int' to 'F'


def dbl(x: Int) -> Int:
    return x * 2


def apply[F: def(Int) -> Int](f: F) -> Int:
    return f(3)


def main():
    var f = dbl
    print(apply(f))
