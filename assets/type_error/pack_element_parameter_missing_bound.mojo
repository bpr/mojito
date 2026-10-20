# expect: does not conform to trait 'Writable'
# A parameter typed by an element of the declaration's own pack has the
# pack's bound in the body, and nothing more: an element of a `Movable` pack
# is not `Writable`, as at the pin.
def show_first[*Ts: Movable](x: Ts[0]):
    print(x)


def main():
    show_first[Int, Bool](7)
