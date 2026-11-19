# expect: that is not applied
# A generic named origin (`UntrackedOrigin[*, mut: Bool]`) is a generator
# until its `mut` is applied: an origin slot takes no unapplied one, as at
# the pin.


struct S[mut: Bool, //, o: Origin[mut=mut]](TrivialRegisterPassable):
    def __init__(out self):
        pass


def main():
    _ = S[UntrackedOrigin]()
