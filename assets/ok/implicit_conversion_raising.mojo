# An `@implicit` constructor may raise: every conversion through it is a
# raising call, handled like any other.
struct Fallible:
    var value: Int

    @implicit
    def __init__(out self, value: Int) raises:
        if value < 0:
            raise Error("neg")
        self.value = value

def use(f: Fallible):
    print(f.value)

def main() raises:
    use(3)
    try:
        use(-1)
    except e:
        print(e)
