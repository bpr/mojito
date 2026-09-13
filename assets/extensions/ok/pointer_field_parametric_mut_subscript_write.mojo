# A write through a pointer field whose origin is parametrically mutable
# (`Origin[mut=m]`): Mojito accepts it in the generic body and judges it per
# instantiation, while the pin insists the destination be provably mutable and
# gives no way to prove `m` — a `where Self.m` clause does not help. The
# `ref`-field spelling lives at
# assets/extensions/ok/ref_field_parametric_mut_subscript_write.mojo.
# Subscript writes through a parametric-mut pointer field (`Origin[mut=m]`)
# are accepted inside the generic body and judged per instantiation: a receiver
# whose origin binds a mutable source may write through the view, and the
# write lands in the borrowed storage.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

    def bump(mut self):
        self.src[][0] += 1

    def put(mut self, x: Int):
        self.src[][0] = x

    def first(self) -> Int:
        return self.src[][0]

def main():
    var data = List[Int]()
    data.append(7)
    var v = View(data)
    v.bump()
    print(v.first())
    v.put(20)
    print(v.first())
    print(data[0])
