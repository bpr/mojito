# A `Pointer[...]` bracket binds over upstream's declared parameter list
# `Pointer[mut: Bool, //, T: AnyType, origin: Origin[mut=mut], *,
# address_space: AddressSpace = .GENERIC]`: keywords bind any slot by name,
# `mut=` only ahead of the positionals, and `mut=False` passes the origin
# as immutable. A bracketed `Pointer[...](to=x)` names the instance the
# place's pointer converts to.
from std.memory import AddressSpace


def address_is_set(p: Pointer) -> Bool:
    return Int(p) != 0


def read[o: Origin](p: Pointer[Int, o]) -> Int:
    return p[]


def main():
    var x = 5
    var p = Pointer(to=x)
    var a: Pointer[T=Int, origin=origin_of(x)] = p
    var b: Pointer[origin=origin_of(x), T=Int] = p
    var c: Pointer[Int, origin=origin_of(x)] = p
    var d: Pointer[mut=True, Int, origin_of(x)] = p
    print(a[], b[], c[], d[])
    var e: Pointer[mut=False, Int, origin_of(x)] = p
    print(e[], e.mut, read(e))
    var f = Pointer[mut=True, Int, origin_of(x)](to=x)
    var g = Pointer[Int, origin_of(x), address_space=AddressSpace.GENERIC](to=x)
    var h = Pointer[T=Int, origin=origin_of(x)](to=x)
    var i = Pointer[origin=origin_of(x)](to=x)
    print(f[], g[], h[], i[])
    var j = Pointer[mut=False, Int, origin_of(x)](to=x)
    var k = Pointer[Int, ImmOrigin(origin_of(x))](to=x)
    var l = Pointer[mut=False](to=x)
    print(j[], j.mut, k[], k.mut, l[], l.mut)
    var m = Pointer[Int, MutAnyOrigin](to=x)
    m[] = 7
    print(x, m[])
    print(address_is_set(p), address_is_set(p.as_imm()))
