# A pointer a method or subscript returns, passed straight to a call, keeps
# the storage its origin names alive through that call, and a field read
# whose type names an origin carries the loans its object holds: `x` is
# destroyed only after the read, as the pin destroys it.
struct Noisy(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


struct View[o: ImmOrigin](TrivialRegisterPassable):
    var p: Pointer[Noisy, Self.o]

    def __init__(out self, p: Pointer[Noisy, Self.o]):
        self.p = p

    def get(self) -> Pointer[Noisy, Self.o]:
        return self.p


def read_ptr(p: Pointer[Noisy, _]) -> Int:
    return p[].v


def read_first(p: Pointer[Int, _]) -> Int:
    return p[]


def method_result():
    var x = Noisy(1)
    var v = View(Pointer(to=x))
    print("v", read_ptr(v.get()))


def span_pointer():
    var xs: List[Int] = [4, 5, 6]
    var s = Span(xs)
    print("first", read_first(s.unsafe_ptr()))


def field_read():
    var x = Noisy(2)
    var v = View(Pointer(to=x))
    var p = v.p
    print("v", p[].v)


def main():
    method_result()
    span_pointer()
    field_read()
