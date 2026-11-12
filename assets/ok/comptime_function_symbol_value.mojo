# A function named where a compile-time value is read is that value, its
# symbol, typed by its signature: a module, local, or struct `comptime`
# binds it, bare or converted by `OptionalReg`'s `@implicit` initializer, a
# compile-time call returns it, and a call through the binding, run-time or
# compile-time, calls the function.
from std.collections import OptionalReg


def sq(x: Int) -> Int:
    return x * x


def mk() -> OptionalReg[def(Int) thin -> Int]:
    return OptionalReg[def(Int) thin -> Int](sq)


comptime h: def(Int) thin -> Int = sq
comptime u = sq
comptime g: OptionalReg[def(Int) thin -> Int] = sq


struct D:
    comptime f: OptionalReg[def(Int) thin -> Int] = sq
    comptime e = OptionalReg[def(Int) thin -> Int](sq)


def main():
    comptime r = h(5)
    print(r)
    print(h(7))
    print(u(3))
    comptime if g:
        print("g holds a function")
    print(g.value()(6))
    comptime a = (D.f.value())(5)
    print(a)
    comptime if D.e:
        print("D.e holds a function")
    print(D.f.value()(4))
    comptime w = mk()
    print(w.value()(3))
    comptime if w:
        print("w holds a function")
    comptime local: def(Int) thin -> Int = sq
    print(local(2))
    comptime lr = local(8)
    print(lr)
