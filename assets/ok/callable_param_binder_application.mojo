# A bracket over a generic callable parameter (`f[4]` inside `run[f:
# def[n: Int]() thin -> Int]`) is upstream's `#kgen.bind_params` over the
# binder: read as a value, at an annotated binding, as a run-time or
# compile-time argument, and in a local `comptime`, it is the specialization
# of the function the instance binds to `f`, the same one `twice[4]` names.


def twice[n: Int]() -> Int:
    return n * 2


def scaled[n: Int, m: Int = 3]() -> Int:
    return n * m


def size[T: AnyType]() -> Int:
    return 7


def go(g: def() thin -> Int) -> Int:
    return g()


def call0[g: def() thin -> Int]() -> Int:
    return g()


def run[f: def[n: Int]() thin -> Int, k: Int]():
    var g = f[4]
    var direct = twice[4]
    var annotated: def() thin -> Int = f[6]
    var over_binder = f[k]
    comptime local = f[2]
    print(g(), direct(), annotated(), over_binder(), f[k + 1]())
    print(go(f[7]), call0[f[5]](), call0[f[6]](), local())


def run_defaulted[f: def[n: Int, m: Int = 3]() thin -> Int]():
    var g = f[4]
    var h = f[4, 5]
    print(g(), h())


def run_type[f: def[T: AnyType]() thin -> Int]():
    var g = f[Int]
    print(g())


def main():
    run[twice, 5]()
    run_defaulted[scaled]()
    run_type[size]()
