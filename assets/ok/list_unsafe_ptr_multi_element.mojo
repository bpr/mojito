# `List.unsafe_ptr()` mints a borrowed multi-element pointer: its
# interior-generation origin permits reads and writes at any element offset
# (the arena bounds check remains the dynamic backstop), keeps the List
# alive while the pointer is used, and `unsafe_offset` stays legal on the
# multi-element domain.
def main():
    var xs: List[Int] = [10, 20, 30]
    var p = xs.unsafe_ptr()
    print(p[0], p[2])
    p[1] = 21
    print(xs[1])
    var q = p.unsafe_offset(2)
    print(q[])
