# expect: write through a Pointer whose origin is immutable
# A pointer field whose origin binder has symbolic mutability
# (`Origin[mut=m]`) is judged per binding: `P(Pointer(to=xs))` from a read
# parameter binds `m` False, so the write rejects as upstream's does.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def show(xs: List[Int]):
    var p = P(Pointer(to=xs))
    p.src[][0] = 9
    print(p.src[][0])

def main():
    var xs = List[Int]()
    xs.append(7)
    show(xs)
