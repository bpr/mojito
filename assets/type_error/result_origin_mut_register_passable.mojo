# `Int` is register-passable, so even a `mut` parameter of it has no memory
# a result could borrow.
# expect: cannot return 'items's origin, because it has RegisterPassable type 'Int'
def alias(mut items: Int) -> ref[origin_of(items)] Int:
    return items


def main():
    var x = 1
    print(alias(x))
