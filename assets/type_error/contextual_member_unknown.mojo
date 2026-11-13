# expect: 'Color' value has no attribute 'nope'
# An unknown member on a successfully resolved base rejects exactly like the
# spelled form `Color.nope()`: the type read as a value has no attribute of
# that name, as at the pin.
@fieldwise_init
struct Color(Copyable, Movable):
    var value: Int

    @staticmethod
    def red() -> Color:
        return Color(1)

def main():
    var c: Color = .nope()
