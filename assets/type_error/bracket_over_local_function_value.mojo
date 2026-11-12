# A bracket after a local holding a function value subscripts that value,
# which a function type does not support: only a declared `def` or a
# compile-time callable binds its parameters through a bracket.
# expect: type 'def() thin -> Int' cannot be indexed here
def twice[n: Int]() -> Int:
    return n * 2


def main():
    var f = twice[3]
    var g = f[4]
    print(g())
