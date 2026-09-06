# A tracked pointer (`Pointer(to=w)`, origin `origin_of(w)`) does not convert
# to a parameter or field declared `Pointer[T, MutUntrackedOrigin]`: as in
# current Mojo ("value passed to 'p' cannot be converted from
# 'Pointer[Int, origin_of(w)]' to 'Pointer[Int, MutUntrackedOrigin]'"), there
# is no implicit widening to an untracked origin — an untracked slot would
# drop the source's keep-alive loan. The explicit
# `unsafe_origin_cast[MutUntrackedOrigin]()` spelling takes that responsibility.
# expect: type mismatch for argument 1 to 'Raw.__init__'

struct Raw[T: Copyable]:
    var p: Pointer[Self.T, MutUntrackedOrigin]

    def __init__(out self, p: Pointer[Self.T, MutUntrackedOrigin]):
        self.p = p


def main():
    var w = 5
    var r = Raw[Int](Pointer(to=w))
    print(r.p[])
