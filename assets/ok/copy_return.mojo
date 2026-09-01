struct Foo(Copyable):
    var s: String

    def __init__(out self, s: String):
        self.s = s

    def __init__(out self, *, copy: Self):
        print("copying value")
        self.s = copy.s


def copy_return[T: Copyable](value: T) -> T:
    return value.copy()


def main():
    var a = Foo("Hello")
    var b = copy_return(a)
    print(b.s)
