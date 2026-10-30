# expect: type mismatch for argument 's' to 'show'
# A converting constructor's own parameter solved from the source must meet
# its bound: `Opaque` is not `Writable`, so `Shown.__init__[T: Writable]`
# does not convert it.
struct Opaque:
    def __init__(out self):
        pass

struct Shown:
    var text: String

    @implicit
    def __init__[T: Writable](out self, value: T):
        self.text = String(value)

def show(s: Shown):
    print(s.text)

def main():
    show(Opaque())
