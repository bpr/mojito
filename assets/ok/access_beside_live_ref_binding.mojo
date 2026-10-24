# Accesses to a place while a `ref` binding over it, or over one of its
# elements, is still live: a second element reference, a temporary `match`
# subject's element bindings, a reference or write taken through the binding
# itself, a `match` case with a `ref` binding reading through the hidden
# subject, the aliasing swap whose desugar binds the source as a hidden
# reference while its leaves write it, an `as` binding beside a binding of one
# of its parts, and a write to the owner itself. A `ref` binding names an
# origin and borrows nothing exclusively, so all of them run.
def tuple_elements():
    var t = (1, 2)
    ref a = t[0]
    ref b = t[1]
    print(a, b)


def temporary_subject():
    match (1, 2):
        case (p, q):
            print("tuple", p, q)
    match (1, 2):
        case [p, q]:
            print("seq", p, q)
        case _:
            print("other")


def through_binding():
    var xs: List[Int] = [1, 2, 3]
    ref a = xs
    ref m = a[1]
    m = 20
    a[0] = 5
    print(xs[0], xs[1])
    var ys: List[Int] = [1, 2, 3]
    ref c = ys
    ref d = c
    d.append(4)
    c.append(5)
    print(len(ys))


def match_ref_case():
    var t = (1, 2, 3)
    match t:
        case (1, ref m, var z):
            m = 20
            z += 30
            print("three", m, z)
        case _:
            print("other")
    print(t[1], t[2])
    var xs: List[Int] = [1, 2, 3]
    match xs:
        case [1, ref m, var z]:
            m = 20
            z += 30
            print("three", m, z)
        case _:
            print("other")
    print(xs[1], xs[2])


def aliasing_swap() raises:
    var xs: List[Int] = [1, 2]
    [xs[1], xs[0]] = xs
    print(xs[0], xs[1])


def as_beside_parts():
    match (1, 2):
        case (v, _) as whole:
            print(v, whole[1])
    var o = Optional(3)
    match o:
        case .Some(v) as whole:
            print(v, whole.value())
        case _:
            print("none")


def owner_writes():
    var kk = 1
    ref r = kk
    kk = 5
    print(r)
    var xs: List[Int] = [1, 2, 3]
    ref c = xs
    xs.append(4)
    print(len(c))
    var s = String("a")
    ref t = s
    s = String("bb")
    print(t)


def main() raises:
    tuple_elements()
    temporary_subject()
    through_binding()
    match_ref_case()
    aliasing_swap()
    as_beside_parts()
    owner_writes()
