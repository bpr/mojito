# expect: no contextual type is available
# Without a contextual type, a leading-dot member reference is an error
# (upstream: "cannot resolve inferred member without a contextual type").
@fieldwise_init
struct Color(Copyable, Movable):
    var value: Int

    @staticmethod
    def red() -> Color:
        return Color(1)

def main():
    var x = .red()
