# `Some[X]` binds an anonymous parameter to the trait value `X` evaluates
# to: a `&` composition (module-qualified or not, or a trait constant)
# bounds it by every trait, and a function type bounds it as a callable.
# The binder is an archetype, so a local copy needs `Deinitable` in the
# bound to be destroyed.
import std.format as fm

comptime WritableCopy = fm.Writable & Copyable


def show(t: Some[fm.Writable & Copyable & Deinitable]):
    var u = t.copy()
    print(u)


def show_again(t: Some[WritableCopy]):
    print(t)


def apply(g: Some[def(var Int)], v: Int):
    g(v)


def make(f: Some[def() -> Int]) -> Int:
    return f()


def fill(fill_fn: Some[def[T: Writer](mut T)]):
    var s = String()
    fill_fn(s)
    print(s)


def twice(var x: Int):
    print(x * 2)


@fieldwise_init
struct Runner:
    var n: Int

    def run(self, g: Some[def(var Int)]):
        g(self.n)


def main():
    show(3)
    show_again("x")
    apply(twice, 4)

    def seven() -> Int:
        return 7

    print(make(seven))

    def write_hi[T: Writer](mut writer: T):
        writer.write("hi ", 3)

    fill(write_hi)
    Runner(6).run(twice)
