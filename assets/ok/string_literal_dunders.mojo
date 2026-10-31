# A string literal's comparisons and `+` are `StringLiteral`'s dunders over
# `StringSlice`, with `String.__radd__` and `StringSpan.__add__`/`__radd__`
# answering a literal or view beside an owned string or another view; two
# literals concatenate to the literal of both texts.
def label[s: StringLiteral]() -> String:
    return String(s + "!")


def main():
    var s = String("ab")
    print("x" + s, "ab" == s, "ab" != s)
    print("ab" < s, "ab" <= s, "b" > s, "a" >= s)
    var t: StaticString = "q"
    print(String("a") + t, t + "z", "w" + t, t + t)
    var name: StaticString = "log"
    print("`" + name + "` y")
    comptime joined = "ab" + "cd"
    print(joined, "x" + "y" + "z", ("p" + "q").byte_length(), label["hi"]())
    print("ab" == "ab", "ab" == "cd", "a" < "b", "b" <= "a")
    var text = String("a")
    text += " " + s
    print(text)
