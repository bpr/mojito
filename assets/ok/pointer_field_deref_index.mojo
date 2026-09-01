# Dereferencing a pointer-to-collection field and indexing the result:
# `v.src[]` is the whole List (an identity reference projection through the
# stored handle), not element 0 of it, so `v.src[][0]` reads the element and
# `len(v.src[])` the length.
@fieldwise_init
struct View[o: Origin[mut=False]]:
    var src: Pointer[List[Int], Self.o]

def main():
    var xs = List[Int]()
    xs.append(3)
    xs.append(5)
    var v = View(Pointer(to=xs))
    print(len(v.src[]))
    print(v.src[][0])
    print(v.src[][1])
