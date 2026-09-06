# The String result APIs live on `StringSpan` (upstream's shape): every
# search, affix, replace, split, case, predicate, justification, and strip
# member runs on a borrowed view receiver — bound or a constructed
# temporary — and needles are views too: a view, a String, or a literal
# argument all convert to the `StringSpan` parameter.
def main():
    var s = String("hello, world")
    var view = s[byte=0:5]
    print(view.find("l"), view.rfind("l"), view.count("l"), "ell" in view, "z" in view)
    var needle = String("el")
    print(view.startswith("he"), view.endswith("lo"), view.startswith(needle, 1), view.endswith(needle, 0, 3))
    var shout = String("WORLD!")
    print(view.replace("l", "L"), view.upper(), shout[byte=0:5].lower())
    print(s.find(shout[byte=0:5].lower()), s.count(view), s.replace(view, "bye"))
    var csv = String("a,b,c")
    var parts = StringSpan(csv).split(",")
    print(len(parts), parts[0], parts[2])
    var limited = StringSpan(csv).split(",", 1)
    var keyword = StringSpan(csv).split(",", maxsplit=1)
    print(len(limited), limited[1], len(keyword), keyword[1])
    var words = String("  one  two ")
    var spaced = StringSpan(words)
    var pieces = spaced.split()
    var bounded = spaced.split(maxsplit=1)
    print(len(pieces), pieces[1], len(bounded), bounded[1])
    var text = String("a\nb\r\nc")
    var lines = StringSpan(text).splitlines()
    print(len(lines), lines[1])
    var padded = String("  mojo  ")
    var trimmed = StringSpan(padded).strip()
    print(trimmed, trimmed.byte_length())
    print(trimmed.removeprefix("mo"))
    print(StringSpan(padded).lstrip().rstrip("o "))
    var blank = String("  \t")
    var digits = String("42")
    print(view.isupper(), view.islower(), StringSpan(blank).isspace(), StringSpan(digits).is_ascii_digit(), view.is_ascii_printable())
    print("[" + view.ascii_rjust(7) + "]", "[" + view.ascii_ljust(7, "*") + "]", "[" + view.ascii_center(8, "-") + "]")
