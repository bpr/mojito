# A bracket over a generic callable parameter binds its arguments against
# the parameter's own declarations, as upstream's `#kgen.bind_params` does: a
# type at the value slot `n: Int` is rejected.
# expect: parameter 'n' has 'Int' type, but value has type 'AnyStruct[Int]'


def twice[n: Int]() -> Int:
    return n * 2


def run[f: def[n: Int]() thin -> Int]():
    var g = f[Int]
    print(g())


def main():
    run[twice]()
