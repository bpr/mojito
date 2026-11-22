# A call through a direct `ref` field (an extension; the pin declares no
# such field) passes the reference the field holds, even when the element
# type carries an origin argument (`StaticString`) that the caller's place
# and the callee's field spell differently: `step` prints `1` and then `7`
# on both backends.
struct Bag[T: Copyable & Deinitable]:
    var items: List[Self.T]
    def __init__(out self, var items: List[Self.T]):
        self.items = items^
    @no_inline
    def get(ref self, i: Int) -> ref[origin_of(self.items)._get_owned_interior["element"]] Self.T:
        return self.items[i]

struct Walk[mut: Bool, //, T: Copyable & Deinitable, o: Origin[mut=mut]]:
    var src: ref[Self.o] Bag[Self.T]
    var index: Int
    def __init__(out self, ref[Self.o] src: Bag[Self.T]):
        self.src = src
        self.index = 0
    def step(mut self) -> Self.T:
        var r = self.index
        self.index += 1
        return self.src.get(r).copy()

def main():
    var l = List[StaticString]()
    l.append("x")
    var b = Bag[StaticString](l^)
    var w = Walk(b)
    print(w.step().byte_length())
    var m = List[Int]()
    m.append(7)
    var c = Bag[Int](m^)
    var v = Walk(c)
    print(v.step())
