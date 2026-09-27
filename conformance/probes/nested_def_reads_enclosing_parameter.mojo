# A nested `def` reading its enclosing function's value parameter (`n`
# inside `inner`) is rejected on both backends: "Could not infer capture
# convention of the captured value n". A parameter is a compile-time value
# the nested body reads without a capture; the pinned Mojo prints 32 and 64.
def scaled[n: Int]() -> Int:
    return n


def outer[n: Int]() -> Int:
    def inner[k: Int]() -> Int:
        return scaled[k + 1]() * n

    return inner[10]() + inner[20]()


def main():
    print(outer[1]())
    print(outer[2]())
