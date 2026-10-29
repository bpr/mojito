# A compile-time parameter of a bare struct type whose parameters are
# infer-only is parameterized over them, on a `def` and on a struct: each is
# solved from the argument's type, or from the default's when the call omits
# the argument.
@fieldwise_init
struct Tag[n: Int, //](ImplicitlyCopyable):
    def get(self) -> Int:
        return Self.n


def show[t: Tag]():
    print(t.get())


def show_default[t: Tag = Tag[n=4]()]():
    print(t.get())


struct Holder[t: Tag]:
    @staticmethod
    def get() -> Int:
        return Self.t.get()


def main():
    show[Tag[n=3]()]()
    show_default()
    show_default[Tag[n=5]()]()
    print(Holder[Tag[n=6]()].get())
