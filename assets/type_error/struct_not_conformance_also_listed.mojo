# A trait may not be both listed and opted out with `not` in one
# conformance list.
# expect: trait 'Movable' must not be listed and also opted out with 'not'; remove one of the entries


struct Handle(Movable, not Movable):
    var fd: Int


def main():
    pass
