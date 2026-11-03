# Upstream's `StringLiteral` members whose bodies read `StringSlice(self)`:
# `StaticString(lit.value)` views the literal's static bytes, and each
# member forwards to the view's.


def main() raises:
    print(StringLiteral["12".value]().__int__(), "2.5".__float__())
    print(Int("42"), Float64("1.5"), "ab".__fspath__())
    print("hello".find("l"), "hello".rfind("l"), "hello".count("l"))
    print("hello".startswith("he"), "hello".endswith("lo"))
    print("Hello".upper(), "Hello".lower(), "ab" * 3)
    print("123".is_ascii_digit(), "ABC".isupper(), "abc".islower())
    print("abc".ascii_rjust(5, "*"), "abc".ascii_ljust(5, "*"))
    print("abc".ascii_center(7, "-"))
    print("[" + " x ".strip() + "]", "[" + "xxaxx".strip("x") + "]")
    print("[" + " x ".lstrip() + "]", "[" + " x ".rstrip() + "]")
    print("a,b".split(","), len("a b  c".split()))
    print("héllo".count_codepoints())
    for g in "abc":
        print(g, end=" ")
    print()
    for g in "abc".__reversed__():
        print(g, end=" ")
    print()
    for c in "ab".codepoints():
        print(c, end=" ")
    print()
    for c in "ab".codepoint_slices():
        print(c, end=" ")
    print()
    var xs: List[Int] = [1, 2, 3]
    print(",".join(xs))
    print("{} and {}".format(1, "x"))
    print("a" == "a", "a" != "b", "a" < "b", "b" >= "a")
    print(repr("a\nb"))
