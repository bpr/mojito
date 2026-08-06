# `for ref` over a temporary List routes through the ordinary
# reference-yielding protocol: the loop retains the temporary in its own slot
# (loop-owned, therefore mutable) and the bindings write through into it.
def make() -> List[Int]:
    return [1, 2, 3]


def main():
    var total = 0
    for ref x in make():
        x += 10
        total += x
    print(total)
