# An `enum` derives `Equatable`, `Comparable`, `Hashable`, and `Writable`
# when it lists them, over its cases' identity: equality and ordering
# compare cases in declaration order, hashing hashes the case, and writing
# writes its name. Its methods use the derived equality, and a leading-dot
# case resolves against a parameter's type.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum Color(Comparable, Hashable, Writable):
    case red, green, blue

    def is_red(self) -> Bool:
        return self == .red


def brush(color: Color) -> String:
    return String(color)


def main() raises:
    var color = Color.green
    var same = color
    print(color == same, color != .blue, color.is_red(), Color.red.is_red())
    print(Color.red < Color.green, Color.blue < Color.green, Color.red <= .red)
    print(hash(Color.red) == hash(Color.red), hash(Color.red) == hash(Color.blue))
    print(brush(.blue), color, repr(Color.red))
    var colors: List[Color] = [.blue, .red, .green]
    var counts = Dict[Color, Int]()
    for c in colors:
        counts[c] = Int(c < Color.blue)
    print(len(counts), counts[Color.red], counts[Color.blue])
