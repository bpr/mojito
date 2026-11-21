# `repr` of a `Pointer` runs upstream's `write_repr_to` from
# `std.memory.pointer`: its `mut`, element type name, and address space,
# then its address. A member and a generic instance over a mutable and an
# immutable pointer are two instances. The address varies, so each line
# prints the text before it.
def show[T: Writable](t: T) -> String:
    return repr(t)

def prefix(r: String) -> String:
    return String(r[byte=0:r.find("(0x")])

def main():
    var x = 5
    var p = Pointer(to=x)
    var q = Pointer(to=x).as_imm()
    print(prefix(repr(p)))
    print(prefix(repr(q)))
    var s = String("hi")
    print(prefix(repr(Pointer(to=s))))
    var r = repr(p)
    print(r.endswith(")"), r[byte=r.find("(") + 1:].startswith("0x"))
    var a = String()
    var b = String()
    p.write_repr_to(a)
    q.write_repr_to(b)
    print(prefix(a))
    print(prefix(b))
    print(prefix(show(p)))
    print(prefix(show(q)))
