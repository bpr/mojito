# A `Pointer` type argument over an interior-projected place
# (`Pointer[Int, origin_of(a)._get_owned_interior["element"]]`, as an
# array's element pointer is) bakes into its clone with a clone origin
# binder that re-applies the projection below the place it stands for:
# every array shares one clone of a user generic `def` and of a generic
# struct's methods, apart from the clone a plain place's pointer shares.


struct Holder[T: Copyable & Deinitable](Copyable):
    var item: Self.T

    def __init__(out self, item: Self.T):
        self.item = item.copy()

    def get(self) -> Self.T:
        return self.item.copy()


def first[T: Copyable & Deinitable](h: Holder[T]) -> T:
    return h.get()


def main():
    var a = Array[Int, 2](fill=3)
    var b = Array[Int, 2](fill=4)
    var x = 5
    var ha = Holder(a.unsafe_ptr())
    var hb = Holder(b.unsafe_ptr())
    var hx = Holder(Pointer(to=x))
    var pa = first(ha)
    var pb = first(hb)
    var px = first(hx)
    print(pa[], pb[], px[], hb.get()[])
    pa[] = 30
    px[] = 50
    print(a[0], x)

