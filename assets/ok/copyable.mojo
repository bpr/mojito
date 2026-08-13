struct Foo(Copyable):
    var s: String

    def __init__(out self, s: String):
        self.s = s

    def __init__(out self, *, copy: Self):
        print("copying value")
        self.s = copy.s

def main():
    var a = Foo("Hello")
    var b = a.copy()
    print(b.s)

