# An entry conditioned on `False` still lists its trait, so opting the same
# trait out with `not` beside it is a trait both listed and opted out.
# expect: trait 'Movable' must not be listed and also opted out with 'not'; remove one of the entries


struct Handle(Movable where False, not Movable):
    var fd: Int


def main():
    pass
