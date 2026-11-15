# A bracket that binds some of a function's parameters defaults the rest,
# as a value and in a call; an empty bracket in a call is the bare call.
def h[b: Int = 3](y: Int) -> Int:
    return y + b


def k[a: Int, b: Int = 3](y: Int) -> Int:
    return y + a + b


def main():
    print(h[](1))
    var g = h[2]
    print(g(1))
    var j = k[2]
    print(j(1))

    def inner[c: Int = 4](y: Int) -> Int:
        return y + c

    print(inner[](1))
