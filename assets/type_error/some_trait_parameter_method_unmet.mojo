# `Some[Self.Tr]` checks its argument against the trait the receiver's
# instance binds `Tr` to: `NW` is no `Writable`.
# expect: invalid call to 'm': no overload matches the supplied arguments
struct NW:
    def __init__(out self):
        pass


struct S[Tr: type_of(AnyType)]:
    def __init__(out self):
        pass

    def m(self, t: Some[Self.Tr]):
        pass


def main():
    var s = S[Writable]()
    s.m(NW())
