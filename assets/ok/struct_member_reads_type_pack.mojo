# A struct's `comptime` members read its own type pack: `Self.Ts.length`,
# `len(Self.Ts)`, and `Self.Ts[i]`, each fixed by the application's elements.
struct Row[*Ts: Movable]:
    comptime width = Self.Ts.length
    comptime count = len(Self.Ts)
    comptime First = Self.Ts[0]
    comptime Last = Self.Ts[Self.width - 1]

    @staticmethod
    def describe() -> Int:
        return Self.width * 10 + Self.count

    @staticmethod
    def first_of(var value: Self.First) -> Self.First:
        return value^


def main():
    print(Row[Int, Bool].width)
    print(Row[Int, Bool, String].count)
    var first: Row[Int, Bool].First = 7
    var last: Row[Int, String, Bool].Last = True
    print(first, last)
    print(Row[Int, Bool].describe())
    print(Row[Bool].describe())
    print(Row[Int, String].first_of(5) + 1)
