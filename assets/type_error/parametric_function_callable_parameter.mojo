# A parametric function is no argument of a non-parametric callable
# parameter: no specialization is inferred from the slot, even when every
# parameter defaults.
# expect: 'apply' parameter 'f' has 'def(Int) thin -> Int' type, but value has type
def h[b: Int = 3](y: Int) -> Int:
    return y + b


def apply[f: def(Int) thin -> Int](x: Int) -> Int:
    return f(x)


def main():
    print(apply[h[]](1))
