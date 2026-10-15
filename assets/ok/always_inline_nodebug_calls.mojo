# Calls to `@always_inline("nodebug")` and `@always_inline("builtin")`
# functions are inlined before elaboration; a call of another shape stays a
# call. Each prints what the call computes either way.


@always_inline("nodebug")
def add_one(x: Int) -> Int:
    return x + 1


@always_inline("nodebug")
def byte_count(s: String) -> Int:
    return s.byte_length()


@always_inline("nodebug")
def consume(var s: String) -> Int:
    s += "!"
    return s.byte_length()


@always_inline("nodebug")
def clamp(x: Int, low: Int, high: Int) -> Int:
    if x < low:
        return low
    if x > high:
        return high
    return x


@always_inline("nodebug")
def triangle(n: Int) -> Int:
    var total = 0
    for i in range(n + 1):
        total += i
    return total


@always_inline("nodebug")
def twice(x: Int) -> Int:
    return add_one(add_one(x)) - 2 + x


@always_inline("builtin")
def square(x: Int) -> Int:
    return x * x


@always_inline("nodebug")
def greet(name: String):
    print("hello,", name)


@always_inline("nodebug")
def shout(var words: List[String]) -> String:
    var joined = String("")
    for word in words:
        joined += word.upper()
    return joined


@always_inline("nodebug")
def bump(mut x: Int):
    x += 1


@always_inline("nodebug")
def scaled[k: Int](x: Int) -> Int:
    return x * k


@always_inline("nodebug")
def checked(x: Int) raises -> Int:
    if x < 0:
        raise Error("negative")
    return x


struct Meter:
    var label: String
    var value: Int

    def __init__(out self, label: String, value: Int):
        self.label = label
        self.value = value

    @always_inline("nodebug")
    def reading(self) -> Int:
        return self.value * 10

    @always_inline("nodebug")
    def title(self) -> String:
        return self.label + ":" + String(self.value)

    @always_inline("nodebug")
    def advance(mut self):
        self.value += 1
        bump(self.value)


def bump_twice(mut x: Int):
    bump(x)
    bump(x)


def generic_sum[n: Int](x: Int) -> Int:
    return add_one(x) + square(n)


def main() raises:
    var text = String("mojo")
    print(add_one(41), byte_count(text), clamp(15, 0, 10), clamp(-3, 0, 10))
    print(consume(text^))
    print(triangle(4), twice(5), square(7))
    greet("inliner")
    var words = List[String]()
    words.append("ab")
    words.append("cd")
    print(shout(words^))
    var meter = Meter("volts", 12)
    print(meter.reading(), meter.title())
    var total = 0
    for i in range(3):
        total += add_one(i) + byte_count(String("xy"))
    print(total)
    try:
        print(add_one(checked(2)))
        print(checked(-1))
    except e:
        print("caught", e)
    var y = 3
    bump(y)
    print(y, scaled[3](y), generic_sum[2](1), generic_sum[3](1))
    bump_twice(y)
    meter.advance()
    bump(meter.value)
    print(y, meter.reading(), meter.title())
