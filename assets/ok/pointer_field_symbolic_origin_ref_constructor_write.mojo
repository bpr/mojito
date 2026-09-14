# A `ref[Self.o]` constructor from a mutable local binds the binder mutable,
# and the field write from `main` goes through.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

    def first(self) -> Int:
        return self.src[][0]

def main():
    var data = List[Int]()
    data.append(7)
    var v = View(data)
    v.src[][0] = 20
    print(v.first())
    print(data[0])
