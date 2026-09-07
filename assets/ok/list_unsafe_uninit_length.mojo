# Upstream's `unsafe_uninit_length` construction and resize for List and
# String: the storage is owned at full length while the caller initializes
# every slot through raw pointer stores; a shrink destroys the written tail
# and a grow forwards written slots through reallocation.
def main():
    var xs = List[Int](unsafe_uninit_length=3)
    var p = xs.unsafe_ptr()
    var i = 0
    while i < 3:
        p.unsafe_offset(i).unsafe_write(i * 10)
        i += 1
    print(len(xs), xs[0], xs[1], xs[2])

    var names = List[String](unsafe_uninit_length=2)
    var q = names.unsafe_ptr()
    q.unsafe_offset(0).unsafe_write(String("a"))
    q.unsafe_offset(1).unsafe_write(String("b"))
    names.resize(unsafe_uninit_length=4)
    var r = names.unsafe_ptr()
    r.unsafe_offset(2).unsafe_write(String("c"))
    r.unsafe_offset(3).unsafe_write(String("d"))
    print(len(names), names[0], names[1], names[2], names[3])
    names.resize(unsafe_uninit_length=1)
    print(len(names), names[0])

    var ys: List[Int] = [1, 2, 3]
    ys.resize(unsafe_uninit_length=5)
    ys[3] = 10
    ys[4] = 20
    print(len(ys), ys[3], ys[4])

    var s = String(unsafe_uninit_length=3)
    var b = s.unsafe_ptr_mut()
    b[0] = UInt8(104)
    b[1] = UInt8(105)
    b[2] = UInt8(33)
    print(s, s.byte_length())
    s.resize(unsafe_uninit_length=5)
    var c = s.unsafe_ptr_mut()
    c[3] = UInt8(63)
    c[4] = UInt8(63)
    print(s)
    s.resize(unsafe_uninit_length=2)
    print(s)
