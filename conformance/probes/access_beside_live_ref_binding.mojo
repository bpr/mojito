# PROBE: an access to a place while a `ref` binding over it (or over one of
# its elements) is still live, made directly or through the binding itself:
# Mojito rejects each part with "access to '...' conflicts with live
# reference '...'"; the pin runs every part.
#
# The pin runs it and prints:
#   1 2
#   tuple 1 2
#   seq 1 2
#   5 20
#   5
#   three 20 33
#   20 3
#   three 20 33
#   20 3
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects it (R1082): the `ref` binding's loan is exclusive, so a
# second element reference, a reference taken through the binding, or a
# `match` whose `ref` case reads two elements through the hidden subject
# conflicts with it.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run access_beside_live_ref_binding.mojo
#         cargo run -- run conformance/probes/access_beside_live_ref_binding.mojo
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


def main():
    tuple_elements()
    temporary_subject()
    through_binding()
    match_ref_case()
