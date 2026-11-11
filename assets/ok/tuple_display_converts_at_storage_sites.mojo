# A tuple display a storage site emits under a tuple type converts each
# element to its element type: a `var` annotation, an assignment, a
# `return`, and a display nested in one of those.
def halves() -> Tuple[Int8, Float32]:
    return (1, 2.5)


def main():
    var t: Tuple[Int8, Float32] = (1, 2.5)
    t = (3, 4.5)
    var n: Tuple[Tuple[Int8, Int8], Int8] = ((1, 2), 3)
    print(t[0], t[1], halves()[1], n[0][1], n[1])
