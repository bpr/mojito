# A pointer's loan stays live across a `for` loop, whose `__next__` call
# leaves through a `try` handler, to the owner's mutation after it. The
# pointer names the list itself, not its elements, so the append runs beside
# it, as at the pin (R1160).
def main():
    var xs = List[Int]()
    xs.append(1)
    var p = Pointer(to=xs)
    for i in range(2):
        print(i)
    xs.append(9)
    print(p[][0])
