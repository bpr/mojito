# A bare parametric `def` converts to no function type, so it is no return
# value of one.
# expect: cannot implicitly convert 'def[T: Intable](T) thin -> Int' value to 'def(Int) thin -> Int'
def as_int[T: Intable](x: T) -> Int:
    return Int(x)


def pick() -> def(Int) thin -> Int:
    return as_int


def main():
    print(pick()(1))
