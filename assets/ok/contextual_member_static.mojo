# Leading-dot contextual member references (upstream 2026-08): a `.member`
# chain resolves its base against the expected type. First-slice surface:
# static METHOD calls with postfix chains, in every expected-type position —
# annotated bindings, call arguments, typed collection-literal elements, and
# return positions.
@fieldwise_init
struct Color(Copyable, Movable):
    var value: Int

    @staticmethod
    def red() -> Color:
        return Color(1)

    @staticmethod
    def of(v: Int) -> Color:
        return Color(v)

    def brighten(self) -> Color:
        return Color(self.value + 100)

def takes_color(c: Color) -> Int:
    return c.value

def make() -> Color:
    return .red()

def main():
    var c: Color = .red()
    print(c.value)
    print(takes_color(.of(7)))
    var xs: List[Color] = [.red(), .of(3)]
    print(xs[1].value)
    var b: Color = .red().brighten()
    print(b.value)
    print(make().value)
