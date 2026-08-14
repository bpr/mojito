# A generic struct field may carry a subtree pointer origin
# (`Self.origin._subtree`), and the origin parameter solves per
# construction: two owners monomorphize independently and each handle
# reads its own storage.
@fieldwise_init
struct Buf:
    var value: Int

@fieldwise_init
struct Watch[mut: Bool, //, origin: Origin[mut=mut]]:
    var data: Pointer[Int, Self.origin._subtree]

def main():
    var a = Buf(3)
    var b = Buf(7)
    var wa = Watch(UnsafePointer(to=a.value).origin_cast[origin_of(a)._subtree]())
    var wb = Watch(UnsafePointer(to=b.value).origin_cast[origin_of(b)._subtree]())
    print(wa.data[] + wb.data[])
