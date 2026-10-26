# Bare, `var`, `ref`, and `as` case bindings over a temporary subject each
# see one value: the temporary is made once, the `var` binding takes it
# without a copy, and it is destroyed once after the case body, as at the
# pin.
struct Box(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def make() -> Box:
    print("make")
    return Box(3)


def main():
    match make():
        case _ as whole:
            print("as", whole.v)
    print("---")
    match make():
        case x:
            print("bare", x.v)
    print("---")
    match make():
        case var x:
            print("var", x.v)
    print("---")
    match make():
        case ref x:
            print("ref", x.v)
    print("end")
