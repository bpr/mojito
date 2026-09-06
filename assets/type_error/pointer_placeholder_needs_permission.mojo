# expect: spell the pointer permission
# A placeholder-origin pointer parameter states its permission through the
# `ImmPointer`/`MutPointer` alias; the bare `Pointer[T, _]` spelling leaves
# it to inference, which Mojito does not do.
def first(bytes: Pointer[UInt8, _], n: Int) -> Int:
    return n

def main():
    var s = String("hello")
    print(first(s.unsafe_ptr(), 5))
