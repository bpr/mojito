# expect: expression must be mutable for in-place operator destination
# A write through a `Pointer[T, Self.o]` field inside the generic body is
# never provably mutable (`m` is symbolic there), so `bump` and `put` both
# reject; upstream rejects the body whatever the instantiation.
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
