# expect: Undefined variable 'Color'
# An unknown member on a successfully resolved base rejects exactly like the
# spelled form `Color.nope()` (whose diagnostic this shares — Mojito's
# member-miss falls through to the value path; upstream reports "'Color'
# value has no attribute 'nope'").
@fieldwise_init
struct Color(Copyable, Movable):
    var value: Int

    @staticmethod
    def red() -> Color:
        return Color(1)

def main():
    var c: Color = .nope()
