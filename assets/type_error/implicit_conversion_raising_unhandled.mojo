# expect: requires a surrounding 'try' block
# A conversion through a raising `@implicit` constructor is a raising call.
struct Fallible:
    var value: Int

    @implicit
    def __init__(out self, value: Int) raises:
        if value < 0:
            raise Error("neg")
        self.value = value

def use(f: Fallible):
    print(f.value)

def main():
    use(3)
