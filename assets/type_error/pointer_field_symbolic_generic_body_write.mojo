# expect: write through a Pointer whose origin mutability is not known here
# A `mut self` method's write through the symbolic-origin pointer field
# rejects in the body: `m` is not provably True there, however the receiver
# was constructed.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

    def put(mut self, x: Int):
        self.src[][0] = x

def show(xs: List[Int]):
    var v = View(xs)
    v.put(9)
    print(xs[0])

def main():
    var data = List[Int]()
    data.append(7)
    show(data)
