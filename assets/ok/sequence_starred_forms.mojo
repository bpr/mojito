# Starred sequence forms over a `List`: leading, trailing, middle, and empty
# remainders; a place target; a nested starred target; `*var`; `|`
# alternatives; a guard; `if case`; a mutation after the remainder's last
# use; and the too-short starred target's raise.
def main() raises:
    var values: List[Int] = [1, 2, 3, 4, 5]
    var a = 0
    var b = 0
    [a, *_, b] = values
    print(a, b)
    var [*lead, last] = values
    print(len(lead), last, lead[0])
    var [*empty, x1, x2, x3, x4, x5] = values
    print(len(empty), x1, x5)
    var [f, *[g, *h]] = values
    print(f, g, len(h))
    var [first, *rest] = values
    print(first, len(rest))
    values.append(6)
    print(len(values))
    var empty_list: List[Int] = []
    match empty_list:
        case [*all]:
            print("all", len(all))
    var one: List[Int] = [9]
    match one:
        case [_, *_, _]:
            print("two or more")
        case [x, *r]:
            print("one", x, len(r))
        case _:
            pass
    var two: List[Int] = [1, 2]
    match two:
        case [p, *m, q]:
            print(p, len(m), q)
        case _:
            pass
    match values:
        case [1, *_] | [*_, 5]:
            print("either")
        case _:
            pass
    match values:
        case [head, *tail] if len(tail) > 10:
            print("long", head)
        case [head, *tail]:
            print("short", head, len(tail), tail[0])
        case _:
            pass
    match values:
        case [var v0, *var vs]:
            print(v0, len(vs))
        case _:
            pass
    if case [*_, 5, z] = values:
        print("if case", z)
    var three: List[Int] = [1, 2, 3]
    try:
        var [_, *_, c1, c2, c3] = three
        print(c1, c2, c3)
    except e:
        print("error:", e)
