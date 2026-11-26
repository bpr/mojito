# Upstream's `Pointer(unsafe_from_address=)` reads an `Int` as a pointer
# through `Pointer(to=address).unsafe_bitcast[Self]()[]`: a pointer built
# from another pointer's integer address reads its pointee, a place's and a
# heap element's alike.


def main():
    var x = 5
    var p = Pointer[Int, origin_of(x)](unsafe_from_address=Int(Pointer(to=x)))
    print(p[])
    var xs = [10, 20, 30]
    var q = Pointer[Int, origin_of(xs)](
        unsafe_from_address=Int(xs.unsafe_ptr().unsafe_offset(2))
    )
    print(q[])
