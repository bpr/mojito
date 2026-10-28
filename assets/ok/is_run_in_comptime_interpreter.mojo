# `__is_run_in_comptime_interpreter` is `True` while the comptime interpreter
# runs a body and `False` in the compiled program, wherever the body is
# reached from: a generic instance, a method, a nested function, or a call
# the interpreted body makes. `math.iota` takes its interpreter branch, as
# the interpreter cannot fold `llvm.stepvector`.
from std.math import iota


def diverge(i: Int) -> Int:
    var t: Int
    if __is_run_in_comptime_interpreter:
        t = i + 40
    else:
        t = i + 1
    return t + 1


def flag() -> Bool:
    return __is_run_in_comptime_interpreter


def through_call() -> Int:
    return 10 if flag() else 20


def scaled[n: Int]() -> Int:
    if not __is_run_in_comptime_interpreter:
        return n
    return n * 100


def nested() -> Int:
    def inner() -> Bool:
        return __is_run_in_comptime_interpreter and True

    return 1 if inner() else 0


struct Probe:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def read(self) -> Int:
        if __is_run_in_comptime_interpreter:
            return self.base * 2
        return self.base


def main():
    var x = __is_run_in_comptime_interpreter
    print(x)
    comptime a = diverge(1)
    print(a, diverge(1))
    comptime f = flag()
    print(f, flag())
    comptime c = through_call()
    print(c, through_call())
    comptime s = scaled[3]()
    print(s, scaled[3]())
    comptime k = nested()
    print(k, nested())
    comptime p = Probe(21).read()
    print(p, Probe(21).read())
    comptime v = iota[DType.int16, 4](3)
    print(v, iota[DType.int16, 4](3))
    comptime w = iota[DType.float32, 4](0.5)
    print(w, iota[DType.float32, 4](0.5))
