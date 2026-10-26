# A capture-list closure capturing only by reference is still its storage
# struct, a run-time value, so it cannot bind a compile-time callable
# parameter.
# expect: cannot use a dynamic value in a parameter list
def apply[origins: OriginSet, //, f: def(Int) capturing[origins] -> Int](a: Int) -> Int:
    return f(a)


def run(a: Int) -> Int:
    var y = 14

    def inner(x: Int) {imm y} -> Int:
        return x + y

    return apply[inner](a)


def main():
    print(run(2))
