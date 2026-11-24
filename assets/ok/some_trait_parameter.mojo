# `Some[Tr]` over a trait-valued binder bounds its anonymous parameter by
# the trait the binder receives: the body assumes the binder's declared
# trait, and each call checks the argument against the bound trait.
@fieldwise_init
struct W(Copyable, Writable):
    var n: Int

    def write_to(self, mut w: Some[Writer]):
        w.write("W(", self.n, ")")


def show[Tr: type_of(Writable)](t: Some[Tr]):
    print(t)


def show_pair[Tr: type_of(Writable)](t: Some[Tr], u: Some[Tr]):
    print(t, u)


def twice[Tr: type_of(Copyable)](t: Some[Tr]) -> Int:
    var c = t.copy()
    _ = c^
    return 2


struct Holder[Tr: type_of(AnyType)]:
    def __init__(out self):
        pass

    def take(self, t: Some[Self.Tr]) -> Int:
        return 1

    def pass_on(self, t: Some[Self.Tr]) -> Int:
        return self.take(t)


def main():
    show[Writable](3)
    show_pair[Writable](W(1), "two")
    show_pair[Writable & Copyable](3, W(4))
    print(twice[Copyable](W(5)))
    print(Holder[Writable]().pass_on(W(6)))
    print(Holder[AnyType]().take(W(7)))
