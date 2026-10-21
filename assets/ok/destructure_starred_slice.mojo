# Starred destructuring and patterns over a `List`: the remainder is a
# `Span` borrowing the list, and a too-short starred target raises.
def main() raises:
    var values: List[Int] = [1, 2, 3, 4, 5]
    var [first, *rest] = values
    print(first, len(rest))
    var [*lead, last] = values
    print(len(lead), last)
    match values:
        case [1, *mid, 5]:
            print(len(mid))
        case _:
            print("no")
    match values:
        case [1, *_, 4]:
            print("yes")
        case [*_, 5]:
            print("ends 5")
    match values:
        case [*all]:
            print(len(all))
    var three: List[Int] = [1, 2, 3]
    try:
        var [a, *_, b, c, d] = three
        print(a, b)
    except e:
        print("error:", e)
