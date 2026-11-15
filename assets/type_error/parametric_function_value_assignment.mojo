# A bare parametric `def` converts to no function type, so assigning it to
# a function-typed binding is rejected.
# expect: cannot implicitly convert 'def[b: Int](Int) thin -> Int' value to 'def(Int) thin -> Int'
def h[b: Int = 3](y: Int) -> Int:
    return y + b


def k(y: Int) -> Int:
    return y


def main():
    var g = k
    g = h
    print(g(1))
