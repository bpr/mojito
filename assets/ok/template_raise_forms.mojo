# A per-instantiation clone inherits its checked template's facts for the
# raises the method grammar names beyond a construction
# (`docs/notes/instantiation-from-template.md`, feature `raises`): a raised
# string literal, which becomes an `Error` by the literal's syntax, and a call
# of a raising sibling method or module function, whose recorded effect
# substitutes. A module-level `def` that raises derives as a method does.
@fieldwise_init
struct SlotError[T: AnyType](ImplicitlyCopyable, Movable, Writable):
    def write_to(self, mut writer: Some[Writer]):
        writer.write("SlotError")

    def write_repr_to(self, mut writer: Some[Writer]):
        self.write_to(writer)


def ensure(ok: Bool) raises:
    if not ok:
        raise "not ready"


struct Slot[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var full: Bool

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.full = True

    def take(mut self) raises -> Self.T:
        if not self.full:
            raise "empty slot"
        self.full = False
        return self.item.copy()

    def relay(mut self) raises -> Self.T:
        return self.take()

    def ready(self) raises -> Bool:
        ensure(self.full)
        return True

    def check(self) raises SlotError[Self.T]:
        if not self.full:
            raise SlotError[Self.T]()

    def checked(self) raises SlotError[Self.T] -> Bool:
        self.check()
        return self.full


def refuse[T: Copyable](x: T, ok: Bool) raises -> Int:
    if not ok:
        raise "refused"
    return 1


def forward[T: Copyable](x: T, ok: Bool) raises -> Int:
    return refuse(x, ok) + 1


def main():
    var a = Slot[Int](7)
    var b = Slot[String]("seven")
    try:
        print(a.relay(), b.take())
        print(a.relay())
    except e:
        print("error:", e)
    try:
        print(b.ready())
    except e:
        print("error:", e)
    try:
        print(a.checked())
    except e:
        print("error:", e)
    try:
        print(forward(1, True), forward(String("s"), True))
        print(forward(2.5, False))
    except e:
        print("error:", e)
