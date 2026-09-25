# A per-instantiation method clone inherits its checked template's facts when
# the method raises (`docs/notes/instantiation-from-template.md`, class
# MethodBody, feature `raises`): a bare `raises` accessor that raises
# `Error("…")`, a typed `raises` accessor returning a reference that raises a
# construction of an error type built over the struct's parameter, and one
# that raises a construction of a closed error type. Whether the operand is
# the declared error type, and whether it is a string, hold alike under every
# instance.
@fieldwise_init
struct SlotError[T: AnyType](ImplicitlyCopyable, Movable, Writable):
    def write_to(self, mut writer: Some[Writer]):
        writer.write("SlotError")

    def write_repr_to(self, mut writer: Some[Writer]):
        self.write_to(writer)


@fieldwise_init
struct Exhausted(ImplicitlyCopyable, Movable, Writable):
    def write_to(self, mut writer: Some[Writer]):
        writer.write("Exhausted")

    def write_repr_to(self, mut writer: Some[Writer]):
        self.write_to(writer)


struct Slot[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var full: Bool
    var uses: Int

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.full = True
        self.uses = 0

    def take(mut self) raises -> Self.T:
        if not self.full:
            raise Error("empty slot")
        self.full = False
        return self.item.copy()

    def peek(ref self) raises SlotError[Self.T] -> ref[origin_of(self.item)] Self.T:
        if not self.full:
            raise SlotError[Self.T]()
        return self.item

    def use(mut self, limit: Int) raises Exhausted -> Int:
        if self.uses >= limit:
            raise Exhausted()
        self.uses += 1
        return self.uses


def main():
    var a = Slot[Int](7)
    var b = Slot[String]("seven")
    try:
        print(a.peek())
    except e:
        print("error:", e)
    try:
        print(b.peek())
    except e:
        print("error:", e)
    try:
        print(a.take(), b.take())
        print(a.take())
    except e:
        print("error:", e)
    try:
        print(b.peek())
    except e:
        print("error:", e)
    try:
        print(a.use(2), a.use(2), b.use(1))
        print(b.use(1))
    except e:
        print("error:", e)
