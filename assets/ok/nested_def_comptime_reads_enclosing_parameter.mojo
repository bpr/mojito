# A compile-time call of a nested `def` runs its generator under the
# enclosing instance's parameters: the enclosing function's, a `comptime
# for` variable, an enclosing nested `def`'s, and a method's `Self`
# parameters. Each instance computes its own value.


def outer[length: Int]():
    def double() -> Int:
        return 2 * length

    def scaled[k: Int]() -> Int:
        return k * length

    def helper() -> Int:
        return length + 1

    def via_helper() -> Int:
        return helper() * 2

    @parameter
    def bumped() -> Int:
        return length + 1

    comptime d = double()
    comptime s = scaled[3]()
    comptime h = via_helper()
    comptime b = bumped()
    print(length, d, s, h, b)


def loop[n: Int]():
    comptime for i in range(n):

        def step() -> Int:
            return i * n

        comptime value = step()
        print("step", value)


def deep[n: Int]():
    def mid() -> Int:
        def leaf() -> Int:
            return n + 100

        comptime l = leaf()
        return l

    comptime m = mid()
    print("deep", m)


def pick[n: Int]():
    def big() -> Bool:
        return n > 2

    comptime if big():
        print("big", n)
    else:
        print("small", n)


@fieldwise_init
struct Box[n: Int]:
    var v: Int

    def scaled[k: Int](self) -> Int:
        def factor() -> Int:
            return Self.n * k

        comptime f = factor()
        return self.v * f


def main():
    outer[4]()
    outer[7]()
    loop[3]()
    deep[5]()
    pick[1]()
    pick[3]()
    print(Box[3](2).scaled[4]())
