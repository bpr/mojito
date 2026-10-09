# A bare leading-dot value member (`takes_color(.red)`) resolves against the
# struct-body `comptime red = Color(1)` associated value of the expected
# parameter type and prints 1, as at the pin.
@fieldwise_init
struct Color(ImplicitlyCopyable, Movable):
    var value: Int

    comptime red = Color(1)

def takes_color(c: Color) -> Int:
    return c.value

def main():
    print(takes_color(.red))
