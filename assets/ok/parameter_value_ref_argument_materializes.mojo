# A struct's own value parameter (`Self.k`, `Self.s`), its `comptime` member
# (`Self.N`), and a type's `comptime` member (`T.S`) bound to a `ref`
# argument materialize into a temporary of the declared type whose address
# the callee borrows, as upstream's `kgen.param.constant` does, so a
# `Named` over one writes `name=value` inside a `FormatStruct` chain.
from std.format._utils import Named, FormatStruct


def show(ref x: Int):
    print(x)


def show_string(ref x: String):
    print(x)


trait HasS:
    comptime S: Int


struct A(HasS):
    comptime S = 9


struct R[k: Int, s: String](Writable):
    comptime N = 3
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.v)

    def write_repr_to(self, mut writer: Some[Writer]):
        FormatStruct(writer, "R").params(
            Named("k", Self.k), Named("s", Self.s), Named("N", Self.N)
        ).fields(self.v)

    def show_parameters(self):
        show(Self.k)
        show_string(Self.s)
        show(Self.N)


def show_member[T: HasS]():
    show(T.S)
    var out = String()
    out.write(Named("S", T.S))
    print(out)


def main():
    var text = String()
    R[1, "hi"](255).write_repr_to(text)
    print(text)
    R[2, "yo"](0).show_parameters()
    show_member[A]()
