# expect: type mismatch for assignment target
# A handwritten initializer's `Self.o` is a fixed binder: storing a pointer to
# a plain read parameter (a fresh place origin, not the declared binder) into
# a `Pointer[T, Self.o]` field is rejected — only a `ref[Self.o]` parameter's
# pointer carries the binder. (The pin rejects the same program at
# construction: "failed to infer parameter 'm'".)
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, xs: List[Int], index: Int):
        self.src = Pointer(to=xs)
        self.index = index

def main():
    var data = List[Int]()
    var v = EntryIter(data, 0)
    print(v.index)
