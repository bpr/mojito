# A struct whose origin binder has symbolic mutability (`Origin[mut=m]`)
# binds it from the place, so `P(Pointer(to=xs))` from a read parameter is
# accepted by both. The write through the stored pointer is where they part:
# the pinned Mojo judges the binder per instantiation and rejects it
# ("expression must be mutable in assignment"); Mojito judges only the
# field's declared symbolic mutability, which it treats as writable.
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
