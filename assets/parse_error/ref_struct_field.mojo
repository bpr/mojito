# expect: 'ref' patterns are only valid on the left side of an assignment
# A struct field cannot be a `ref`: the pin rejects the declaration at parse
# with this sentence, and reference storage is spelled `Pointer[T, origin]`.
struct Holder[o: ImmOrigin]:
    var value: ref[Self.o] Int

    def __init__(out self, ref[Self.o] v: Int):
        self.value = v


def main():
    var x = 1
    var h = Holder(x)
    print(h.value)
