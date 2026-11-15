# A parametric function is no default of a non-parametric callable
# parameter.
# expect: type mismatch for callable parameter default
def h[b: Int = 3](y: Int) -> Int:
    return y + b


def run[f: def(Int) thin -> Int = h](x: Int) -> Int:
    return f(x)


def main():
    print(run(1))
