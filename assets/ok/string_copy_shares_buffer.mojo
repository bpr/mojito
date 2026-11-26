# A copy of a String that owns a heap buffer shares the buffer under its
# reference count (the same `unsafe_ptr()`), and a mutation of either copy
# reallocates first, leaving the other's text alone. Inline and static
# Strings keep their forms.
def show(name: String, s: String):
    print(name, s, s.byte_length(), s.capacity_bytes())


def main():
    var h = String(capacity_bytes=26)
    h += "abcdefghijklmnopqrstuvwxyz"
    var c = h.copy()
    print(h.unsafe_ptr() == c.unsafe_ptr())
    c += "!"
    print(h.unsafe_ptr() == c.unsafe_ptr())
    show("h", h)
    show("c", c)
    # copy then mutate the original
    var d = h.copy()
    h += "?"
    print(h.unsafe_ptr() == d.unsafe_ptr())
    show("h", h)
    show("d", d)
    # many copies, dropped in turn
    var xs = List[String]()
    for _ in range(4):
        xs.append(d.copy())
    print(xs[0].unsafe_ptr() == d.unsafe_ptr(), xs[3].unsafe_ptr() == d.unsafe_ptr())
    _ = xs.pop()
    xs[0] += "#"
    show("x0", xs[0])
    show("x1", xs[1])
    show("d", d)
    # implicit copy through a var binding
    var e = d
    print(e.unsafe_ptr() == d.unsafe_ptr())
    # resize / append on a shared buffer
    var f = d.copy()
    f.resize(3)
    show("f", f)
    show("d", d)
    # reserve on shared
    var g = d.copy()
    g.reserve_bytes(100)
    print(g.unsafe_ptr() == d.unsafe_ptr(), g.capacity_bytes())
    # an inline and a static copy stay as they were
    var s = String("hi")
    var t = s.copy()
    print(s.unsafe_ptr() == t.unsafe_ptr())
    var u = String("ab") + "c"
    var v = u.copy()
    print(u.unsafe_ptr() == v.unsafe_ptr())
