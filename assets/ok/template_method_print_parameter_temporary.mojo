# A per-instantiation method clone inherits its checked template's facts when
# the body prints a temporary of a struct parameter's type — a bound builtin's
# copy, or a call through a bound whose result is the requirement's binder
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `print_call`). The builtin records the temporary as unconsumed by its syntax
# alone; that it is `Writable`, and how it is written and dropped, each
# instance settles at its own type.
trait Merger:
    def echo[T: Copyable](self, other: T) -> T:
        ...


@fieldwise_init
struct Count(Copyable, Merger):
    var n: Int

    def echo[T: Copyable](self, other: T) -> T:
        return other.copy()


@fieldwise_init
struct Tag(Copyable, Writable):
    var id: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("#", self.id)


struct Holder[S: Merger & Copyable & Deinitable, W: Writable & Copyable & Deinitable](Movable):
    var s: Self.S
    var w: Self.W

    def __init__(out self, var s: Self.S, var w: Self.W):
        self.s = s^
        self.w = w^

    def copied(self):
        print(self.w.copy())

    def echoed(self):
        print(self.s.echo(self.w))

    def mixed(self):
        print("w =", self.w.copy(), self.s.echo(self.w))

    def nested(self):
        print(self.s.echo(self.w.copy()))

    def parameter(self, other: Self.W):
        print(other.copy(), self.s.echo(other))


def main():
    var a = Holder[Count, Int](Count(1), 3)
    var b = Holder[Count, Bool](Count(1), True)
    var c = Holder[Count, String](Count(1), String("ab"))
    var d = Holder[Count, Tag](Count(1), Tag(7))
    a.copied(); b.copied(); c.copied(); d.copied()
    a.echoed(); b.echoed(); c.echoed(); d.echoed()
    a.mixed(); b.mixed(); c.mixed(); d.mixed()
    a.nested(); b.nested(); c.nested(); d.nested()
    a.parameter(4); b.parameter(False); c.parameter(String("c")); d.parameter(Tag(12))
